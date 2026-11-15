package main

import (
	"fmt"
	"os"

	engine "github.com/listenbox/client/crates/sync-engine"
	p "github.com/listenbox/client/crates/sync-engine/publicapi"
	"github.com/spf13/cobra"
)

const (
	episodePageSize = 100
)

func (app *application) episodeCommands() *cobra.Command {
	episodes := &cobra.Command{Use: "episodes", Short: "Manage episodes"}
	list := &cobra.Command{
		Use:  listCommand,
		Args: cobra.NoArgs,
		RunE: app.listEpisodes,
	}
	list.Flags().String(showFlag, "", "Podcast slug")
	list.Flags().Int("limit", episodePageSize, "Episode page size (1–500)")
	_ = list.MarkFlagRequired(showFlag)
	create := &cobra.Command{
		Use:  "create",
		Args: cobra.NoArgs,
		RunE: app.createEpisode,
	}
	for _, name := range []string{showFlag, titleFlag, "description", "file"} {
		create.Flags().String(name, "", name)
	}
	create.Flags().String("publication", "draft", "draft or publish")
	for _, name := range []string{showFlag, titleFlag, "file"} {
		_ = create.MarkFlagRequired(name)
	}
	deleteCommand := &cobra.Command{
		Use:  "delete",
		Args: cobra.NoArgs,
		RunE: app.deleteEpisode,
	}
	deleteCommand.Flags().String("episode", "", "Episode ID")
	deleteCommand.Flags().Bool("yes", false, "Confirm deletion")
	_ = deleteCommand.MarkFlagRequired("episode")
	episodes.AddCommand(list, create, deleteCommand)
	return episodes
}

func (app *application) listEpisodes(c *cobra.Command, _ []string) error {
	show, err := showSelector(c)
	if err != nil {
		return err
	}
	limit, _ := c.Flags().GetInt("limit")
	if limit < 1 || limit > 500 {
		return fmt.Errorf("--limit %d must be between 1 and 500", limit)
	}
	episodes, err := app.api.Episodes(c.Context(), show, limit)
	if err != nil {
		return err
	}
	for _, episode := range episodes {
		if err := requireID(episode.Id, "ep_"); err != nil {
			return err
		}
		fmt.Println(episode.Id)
	}
	return nil
}

func (app *application) createEpisode(c *cobra.Command, _ []string) error {
	show, err := showSelector(c)
	if err != nil {
		return err
	}
	title, err := engine.Nonblank(flagString(c, titleFlag))
	if err != nil {
		return err
	}
	publication := p.EpisodePublication(flagString(c, "publication"))
	if publication != p.EpisodePublicationDraft &&
		publication != p.EpisodePublicationPublish {
		return fmt.Errorf("invalid --publication %q", publication)
	}
	return app.api.CreateEpisode(
		c.Context(),
		engine.EpisodeCreate{
			Show:        show,
			Title:       title,
			Description: optionalFlag(c, "description"),
			File:        flagString(c, "file"),
			Publication: publication,
		},
		os.Stdout,
		os.Stderr,
	)
}

func (app *application) deleteEpisode(c *cobra.Command, _ []string) error {
	episode := flagString(c, "episode")
	if err := requireID(episode, "ep_"); err != nil {
		return err
	}
	if err := mustConfirm(c); err != nil {
		return err
	}
	var created struct {
		Run string `json:"episode_deletion_run_id"`
	}
	if err := app.api.JSON(
		c.Context(),
		"POST",
		"/s/episodes/"+episode+"/deletions",
		nil,
		&created,
	); err != nil {
		return err
	}
	return app.deleteEvents(
		c.Context(),
		"/s/episode-deletions/"+created.Run+"/events",
		episodeDeletion(episode),
	)
}
