package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"

	engine "github.com/listenbox/client/crates/sync-engine"
	p "github.com/listenbox/client/crates/sync-engine/publicapi"
)

func (app *application) importRSS(ctx context.Context, source, slug string) error {
	body := p.ImportRSSRequest{SourceUrl: source}
	if slug != "" {
		body.Slug = &slug
	}
	var created struct {
		Run string `json:"import_run_id"`
	}
	err := app.api.JSON(ctx, "POST", "/s/rss-imports", body, &created)
	if err != nil {
		return app.rssConflict(ctx, slug, err)
	}
	return app.rssEvents(ctx, created.Run)
}

func (app *application) deleteEvents(
	ctx context.Context,
	path string,
	target deletionTarget,
) error {
	events, err := app.api.Events(ctx, path)
	if err != nil {
		return err
	}
	defer events.Close()
	percent := -1.0
	for {
		event, err := events.Next()
		if err != nil {
			return err
		}
		switch event["type"] {
		case "progress":
			if _, err := engine.EventString(event, "current_step"); err != nil {
				return err
			}
			value, ok := event["percent"].(float64)
			if !ok || value < 0 || value > 100 {
				return errors.New("malformed deletion progress")
			}
			if value > percent {
				percent = value
				fmt.Fprintf(os.Stderr, "Import progress: %.0f%%\n", value)
			}
		case "terminal":
			return completeDeletion(event, target)
		default:
			return fmt.Errorf("malformed deletion event %q", event["type"])
		}
	}
}

func (app *application) rssConflict(ctx context.Context, slug string, err error) error {
	var failure *engine.HTTPError
	if !errors.As(err, &failure) || failure.Status != 409 {
		return err
	}
	var conflict p.PodcastCreationConflict
	if json.Unmarshal(failure.Body, &conflict) != nil || conflict.ShowAlreadyImported == nil {
		if slug != "" {
			return fmt.Errorf("requested slug %q conflicts", slug)
		}
		return errors.New("import choice conflicts")
	}
	existing := conflict.ShowAlreadyImported
	team, err := app.api.TeamID(ctx)
	if err != nil {
		return err
	}
	if existing.TeamId != team {
		return errors.New("RSS import conflict returned another team's podcast")
	}
	location, err := app.config.ShowURL(existing.TeamId, existing.ShowId)
	if err != nil {
		return err
	}
	fmt.Println(existing.ShowSlug)
	fmt.Fprintln(os.Stderr, "Open in Listenbox: "+location)
	return nil
}

func (app *application) rssTerminal(event map[string]any) error {
	switch event["status"] {
	case "completed":
		showSlug, err := engine.EventString(event, "show_slug")
		if err != nil {
			return err
		}
		if _, err := engine.Slug(showSlug); err != nil {
			return err
		}
		feed, err := engine.EventString(event, "feed_id")
		if err != nil || !engine.ValidID(feed, "lb_") {
			return errors.New("invalid import feed ID")
		}
		team, _ := engine.EventString(event, "team_id")
		show, _ := engine.EventString(event, "show_id")
		location, err := app.config.ShowURL(team, show)
		if err != nil {
			return err
		}
		fmt.Fprintln(os.Stderr, "Import progress: 100%")
		fmt.Println(showSlug)
		fmt.Fprintln(os.Stderr, "Open in Listenbox: "+location)
		return nil
	case "cancelled":
		return errors.New("RSS import was cancelled")
	case "failed":
		return fmt.Errorf(
			"RSS import failed (%s): %s",
			event["error_code"],
			event["error_message"],
		)
	default:
		return fmt.Errorf("malformed import terminal status %q", event["status"])
	}
}

type deletionTarget struct{ label, eventKey, identity string }

func showDeletion(slug string) deletionTarget { return deletionTarget{"show", "show_slug", slug} }

func episodeDeletion(
	id string,
) deletionTarget {
	return deletionTarget{"episode", "episode_id", id}
}

func completeDeletion(event map[string]any, target deletionTarget) error {
	if event["status"] != "completed" {
		return fmt.Errorf(
			"%s deletion failed (%s): %s",
			target.label,
			event["error_code"],
			event["error_message"],
		)
	}
	if event[target.eventKey] != target.identity {
		return fmt.Errorf("deletion completed for another %s", target.label)
	}
	fmt.Fprintln(os.Stderr, "Import progress: 100%")
	fmt.Printf("Deleted %s %q\n", target.label, target.identity)
	return nil
}

func (app *application) rssEvents(ctx context.Context, run string) error {
	events, err := app.api.Events(ctx, "/s/rss-imports/"+run+"/events")
	if err != nil {
		return err
	}
	defer events.Close()
	percent := -1.0
	for {
		event, err := events.Next()
		if err != nil {
			return err
		}
		switch event["type"] {
		case "progress":
			value, ok := event["percent"].(float64)
			if !ok || value < 0 || value > 100 {
				return errors.New("malformed import progress")
			}
			if value > percent {
				percent = value
				fmt.Fprintf(os.Stderr, "Import progress: %.0f%%\n", value)
			}
		case "terminal":
			return app.rssTerminal(event)
		default:
			return fmt.Errorf("malformed import event %q", event["type"])
		}
	}
}
