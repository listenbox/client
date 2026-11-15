package syncengine

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"net/url"
	"path/filepath"
	"slices"
	"strings"

	p "github.com/listenbox/client/crates/sync-engine/publicapi"
)

var scopes = []p.ApiKeyScope{
	"team:read",
	"team:manage",
	"show:read",
	"show:create",
	"show:update",
	"episode:create",
	"episode:delete",
	"episode:update",
	"episode:publish",
}

func (api *API) Login(ctx context.Context, client p.AuthorizationClient, open func(string)) error {
	created, err := api.createAuthorization(ctx, client)
	if err != nil {
		return err
	}
	verification, err := api.verificationURL(created.VerificationUrl)
	if err != nil {
		return err
	}
	open(verification)
	events, err := api.Events(ctx, "/cli/authorizations/"+url.PathEscape(created.Code)+"/events")
	if err != nil {
		return err
	}
	defer events.Close()
	event, err := events.Next()
	if err != nil {
		return err
	}
	switch event["type"] {
	case "cli.authorization.approved":
		api.Credential = created.Credential
		if err := api.checkApprovedIdentity(ctx, event["team_id"]); err != nil {
			return err
		}
		return WritePrivateJSON(
			filepath.Join(api.Config.Directory, "auth.json"),
			StoredAuth{APIKey: created.Credential},
		)
	case "cli.authorization.denied":
		//nolint:staticcheck // Preserve existing end-user guidance.
		return errors.New(
			"Authorization was denied",
		)
	case "cli.authorization.expired":
		//nolint:staticcheck // Preserve existing end-user guidance.
		return errors.New(
			"Authorization expired",
		)
	default:
		return fmt.Errorf("unexpected authorization event %q", event["type"])
	}
}

func (api *API) AuthStatus(ctx context.Context) (string, error) {
	var identity p.APIKeyWhoami
	if err := api.JSON(ctx, "GET", "/s/whoami", nil, &identity); err != nil {
		return "", err
	}
	email := strings.TrimSpace(identity.Email)
	if email == "" {
		return "", errors.New("identity email is empty")
	}
	name := ""
	if identity.Name != nil {
		name = strings.Join(strings.Fields(*identity.Name), " ")
	}
	if name != "" {
		return fmt.Sprintf("Logged in as %s <%s>", name, email), nil
	}
	return "Logged in as " + email, nil
}

func (api *API) createAuthorization(
	ctx context.Context,
	client p.AuthorizationClient,
) (p.CreatedCLIAuthorization, error) {
	var created p.CreatedCLIAuthorization
	err := api.JSON(
		ctx,
		"POST",
		"/cli/authorizations",
		p.CreateCLIAuthorization{Client: client, Scopes: scopes},
		&created,
	)
	if err != nil && api.Credential != "" && isStatus(err, http.StatusUnauthorized) {
		api.Credential = ""
		err = api.JSON(
			ctx,
			"POST",
			"/cli/authorizations",
			p.CreateCLIAuthorization{Client: client, Scopes: scopes},
			&created,
		)
	}
	if err != nil {
		return created, err
	}
	if !CredentialValid(created.Credential) {
		return created, errors.New("invalid pending credential")
	}
	return created, nil
}

func (api *API) checkApprovedIdentity(ctx context.Context, team any) error {
	var identity p.APIKeyWhoami
	if err := api.JSON(ctx, "GET", "/s/whoami", nil, &identity); err != nil {
		return err
	}
	if identity.TeamId != team {
		return errors.New("approved credential team does not match approved team")
	}
	for _, scope := range scopes {
		if !slices.Contains(identity.Scopes, scope) {
			return fmt.Errorf("approved credential missing scope %q", scope)
		}
	}
	return nil
}

func (api *API) verificationURL(raw string) (string, error) {
	verification, err := url.Parse(raw)
	if err != nil || verification == nil || verification.Hostname() == "" ||
		verification.User != nil ||
		verification.Fragment != "" ||
		(verification.Scheme != httpsScheme && verification.Scheme != httpScheme) {
		return "", errors.New("invalid verification URL")
	}
	dashboard, _ := url.Parse(api.Config.DashboardOrigin)
	verification.Scheme, verification.Host = dashboard.Scheme, dashboard.Host
	return verification.String(), nil
}
