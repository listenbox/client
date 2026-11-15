package main

import (
	"context"
	"errors"
	"fmt"
	"net/url"
	"strings"

	"github.com/google/uuid"
	engine "github.com/listenbox/client/crates/sync-engine"
	p "github.com/listenbox/client/crates/sync-engine/publicapi"
	"github.com/spf13/cobra"
)

func (app *application) showCommands() *cobra.Command {
	shows := &cobra.Command{Use: "shows", Short: "Manage shows by slug"}
	list := &cobra.Command{
		Use:  listCommand,
		Args: cobra.NoArgs,
		RunE: app.listShows,
	}
	create := &cobra.Command{
		Use:  "create",
		Args: cobra.NoArgs,
		RunE: app.createShow,
	}
	for _, name := range []string{
		titleFlag, slugFlag, "type", "language", "artwork", "apple-podcasts-url", "spotify-url",
	} {
		create.Flags().String(name, "", name)
	}
	for _, name := range []string{titleFlag, slugFlag, "type", "language"} {
		_ = create.MarkFlagRequired(name)
	}
	update := &cobra.Command{
		Use:  "update",
		Args: cobra.NoArgs,
		RunE: app.updateShow,
	}
	for _, name := range []string{
		showFlag, titleFlag, slugFlag, "description", "artwork", "image",
		"apple-podcasts-url", "spotify-url",
	} {
		update.Flags().String(name, "", name)
	}
	_ = update.MarkFlagRequired(showFlag)
	order := &cobra.Command{
		Use:  "order",
		Args: cobra.NoArgs,
		RunE: app.orderEpisodes,
	}
	order.Flags().String(showFlag, "", "Podcast slug")
	order.Flags().StringArray("episode", nil, "Episode ID to move to the front")
	_ = order.MarkFlagRequired(showFlag)
	deleteCommand := &cobra.Command{
		Use:  "delete",
		Args: cobra.NoArgs,
		RunE: app.deleteShow,
	}
	deleteCommand.Flags().String(showFlag, "", "Podcast slug")
	deleteCommand.Flags().Bool("yes", false, "Confirm deletion")
	_ = deleteCommand.MarkFlagRequired(showFlag)
	shows.AddCommand(list, create, update, order, deleteCommand)
	return shows
}

func (app *application) listShows(c *cobra.Command, _ []string) error {
	shows, err := app.shows(c.Context())
	if err != nil {
		return err
	}
	for _, show := range shows {
		fmt.Println(show.Slug)
	}
	return nil
}

func (app *application) createShow(c *cobra.Command, _ []string) error {
	title, err := engine.Nonblank(flagString(c, titleFlag))
	if err != nil {
		return err
	}
	slug, err := engine.Slug(flagString(c, slugFlag))
	if err != nil {
		return err
	}
	language, err := engine.Language(flagString(c, "language"))
	if err != nil {
		return err
	}
	kind := p.ShowSourceKind(flagString(c, "type"))
	if kind != p.ShowSourceKindAudio && kind != p.ShowSourceKindVideo {
		return fmt.Errorf("invalid --type %q; use audio or video", kind)
	}
	id := "shw_" + strings.ReplaceAll(uuid.NewString(), "-", "")[:16]
	image, err := app.artworkFile(c.Context(), id, flagString(c, "artwork"))
	if err != nil {
		return err
	}
	team, err := app.api.TeamID(c.Context())
	if err != nil {
		return err
	}
	var show p.Show
	err = app.api.JSON(
		c.Context(),
		"POST",
		"/s/shows",
		p.CreateShow{
			Id:               id,
			TeamId:           team,
			Title:            title,
			Slug:             slug,
			SourceKind:       kind,
			Language:         language,
			ImageAssetId:     image,
			ApplePodcastsUrl: optionalFlag(c, "apple-podcasts-url"),
			SpotifyUrl:       optionalFlag(c, "spotify-url"),
		},
		&show,
	)
	if err != nil {
		var conflict *engine.HTTPError
		if errors.As(err, &conflict) && conflict.Status == 409 {
			return fmt.Errorf("slug %q already exists", slug)
		}
		return err
	}
	location, err := app.config.ShowURL(show.TeamId, show.Id)
	if err != nil {
		return err
	}
	fmt.Printf("Created show %q\nOpen in Listenbox: %s\n", show.Slug, location)
	return nil
}

func (app *application) updateShow(c *cobra.Command, _ []string) error {
	show, err := showSelector(c)
	if err != nil {
		return err
	}
	body, err := showUpdateMetadata(c)
	if err != nil {
		return err
	}
	path := flagString(c, "artwork")
	if path == "" {
		path = flagString(c, "image")
	}
	if path != "" {
		image, err := app.updatedShowArtwork(c.Context(), show, path)
		if err != nil {
			return err
		}
		body.ImageAssetId = image
	}
	if !hasShowUpdate(body) {
		return errors.New("shows update requires at least one metadata field")
	}
	if err := app.api.JSON(
		c.Context(),
		"PUT",
		"/s/shows/"+url.PathEscape(show),
		body,
		nil,
	); err != nil {
		return err
	}
	fmt.Printf("Updated show %q\n", show)
	return nil
}

func (app *application) orderEpisodes(c *cobra.Command, _ []string) error {
	show, err := showSelector(c)
	if err != nil {
		return err
	}
	ids, _ := c.Flags().GetStringArray("episode")
	for _, id := range ids {
		if err := requireID(id, "ep_"); err != nil {
			return err
		}
	}
	var result p.EpisodeOrder
	if len(ids) == 0 {
		err = app.api.JSON(
			c.Context(),
			"GET",
			"/s/shows/"+show+"/episode-order",
			nil,
			&result,
		)
	} else {
		result, err = app.api.SetOrder(c.Context(), show, nil, ids)
	}
	if err != nil {
		return err
	}
	for _, id := range result.EpisodeIds {
		fmt.Println(id)
	}
	return nil
}

func (app *application) deleteShow(c *cobra.Command, _ []string) error {
	show, err := showSelector(c)
	if err != nil {
		return err
	}
	if err := mustConfirm(c); err != nil {
		return err
	}
	var created struct {
		Run string `json:"show_deletion_run_id"`
	}
	if err := app.api.JSON(
		c.Context(),
		"POST",
		"/s/shows/"+show+"/deletions",
		nil,
		&created,
	); err != nil {
		return err
	}
	return app.deleteEvents(
		c.Context(),
		"/s/show-deletions/"+created.Run+"/events",
		showDeletion(show),
	)
}

func showUpdateMetadata(c *cobra.Command) (p.UpdateShow, error) {
	title := optionalFlag(c, titleFlag)
	if title != nil {
		value, err := engine.Nonblank(*title)
		if err != nil {
			return p.UpdateShow{}, err
		}
		title = &value
	}
	slug := optionalFlag(c, slugFlag)
	if slug != nil {
		value, err := engine.Slug(*slug)
		if err != nil {
			return p.UpdateShow{}, err
		}
		slug = &value
	}
	return p.UpdateShow{
		Title:            title,
		Slug:             slug,
		Description:      optionalFlag(c, "description"),
		ApplePodcastsUrl: optionalFlag(c, "apple-podcasts-url"),
		SpotifyUrl:       optionalFlag(c, "spotify-url"),
	}, nil
}

func (app *application) updatedShowArtwork(
	ctx context.Context,
	slug, path string,
) (*string, error) {
	listed, err := app.shows(ctx)
	if err != nil {
		return nil, err
	}
	id := ""
	for _, existing := range listed {
		if existing.Slug == slug {
			id = existing.Id
		}
	}
	if id == "" {
		return nil, fmt.Errorf("show %q not found", slug)
	}
	image, err := app.api.UploadArtworkFile(ctx, id, path)
	if err != nil {
		return nil, err
	}
	return &image, nil
}

func hasShowUpdate(body p.UpdateShow) bool {
	return body.Title != nil || body.Slug != nil || body.Description != nil ||
		body.ApplePodcastsUrl != nil ||
		body.SpotifyUrl != nil ||
		body.ImageAssetId != nil
}

func (app *application) artworkFile(ctx context.Context, id, path string) (*string, error) {
	if path == "" {
		return nil, nil
	}
	image, err := app.api.UploadArtworkFile(ctx, id, path)
	if err != nil {
		return nil, err
	}
	return &image, nil
}
