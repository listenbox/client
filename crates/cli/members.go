package main

import (
	"fmt"
	"net/url"
	"strings"
	"time"

	p "github.com/listenbox/client/crates/sync-engine/publicapi"
	"github.com/spf13/cobra"
)

type memberTarget struct{ base, show, label string }

func (app *application) memberCommands() *cobra.Command {
	members := &cobra.Command{Use: "members", Short: "Manage team or show members"}
	list := &cobra.Command{Use: listCommand, Args: cobra.NoArgs, RunE: app.listMembers}
	invite := &cobra.Command{Use: inviteCommand, Args: cobra.NoArgs, RunE: app.inviteMember}
	role := &cobra.Command{Use: "role", Args: cobra.NoArgs, RunE: app.changeMemberRole}
	remove := &cobra.Command{Use: removeCommand, Args: cobra.NoArgs, RunE: app.removeMember}
	for _, command := range []*cobra.Command{list, invite, role, remove} {
		command.Flags().String(showFlag, "", "Podcast slug, or omit for team membership")
		members.AddCommand(command)
	}
	invite.Flags().String("email", "", "Member email")
	_ = invite.MarkFlagRequired("email")
	for _, command := range []*cobra.Command{invite, role} {
		command.Flags().String("role", "", "read or write")
		_ = command.MarkFlagRequired("role")
	}
	for _, command := range []*cobra.Command{role, remove} {
		command.Flags().String("member", "", "Member user ID")
		_ = command.MarkFlagRequired("member")
	}
	remove.Flags().Bool("yes", false, "Confirm removal")
	return members
}

func membershipTarget(c *cobra.Command) (memberTarget, error) {
	show := flagString(c, showFlag)
	if show == "" {
		return memberTarget{base: "/s/team", label: "member"}, nil
	}
	value, err := showSelector(c)
	if err != nil {
		return memberTarget{}, err
	}
	return memberTarget{
		base:  "/s/shows/" + url.PathEscape(value),
		show:  show,
		label: "show member",
	}, nil
}

func membershipRole(c *cobra.Command) (p.AssignableTeamRole, error) {
	role := p.AssignableTeamRole(flagString(c, "role"))
	if role != p.AssignableTeamRoleRead && role != p.AssignableTeamRoleWrite {
		return "", fmt.Errorf("invalid --role %q; use read or write", role)
	}
	return role, nil
}

func (app *application) listMembers(c *cobra.Command, _ []string) error {
	target, err := membershipTarget(c)
	if err != nil {
		return err
	}
	if target.show != "" {
		return app.listShowMembers(c, target)
	}
	var collection p.TeamMemberCollection
	if err := app.api.JSON(
		c.Context(),
		"GET",
		target.base+"/members",
		nil,
		&collection,
	); err != nil {
		return err
	}
	for _, member := range collection.Members {
		fmt.Printf("%s\t%s\t%s\n", member.Role, member.User.Email, member.User.Id)
	}
	for _, invitation := range collection.Invitations {
		fmt.Printf(
			"pending\t%s\t%s\t%s\t%s\n",
			invitation.Role,
			invitation.Email,
			invitation.Id,
			time.UnixMilli(invitation.ExpiresAt).UTC().Format(time.RFC3339),
		)
	}
	return nil
}

func (app *application) listShowMembers(c *cobra.Command, target memberTarget) error {
	var collection p.ShowMemberCollection
	if err := app.api.JSON(
		c.Context(),
		"GET",
		target.base+"/members",
		nil,
		&collection,
	); err != nil {
		return err
	}
	for _, member := range collection.Members {
		fmt.Printf("%s\t%s\t%s\t%s\n", member.AccessSource, member.Role, member.User.Email, member.User.Id)
	}
	for _, invitation := range collection.Invitations {
		fmt.Printf(
			"pending\t%s\t%s\t%s\t%s\n",
			invitation.Role,
			invitation.Email,
			invitation.Id,
			time.UnixMilli(invitation.ExpiresAt).UTC().Format(time.RFC3339),
		)
	}
	return nil
}

func (app *application) inviteMember(c *cobra.Command, _ []string) error {
	target, err := membershipTarget(c)
	if err != nil {
		return err
	}
	role, err := membershipRole(c)
	if err != nil {
		return err
	}
	email := strings.ToLower(strings.TrimSpace(flagString(c, "email")))
	if !strings.Contains(email, "@") {
		return fmt.Errorf("invalid --email %q", email)
	}
	var invitation struct {
		Email string `json:"email"`
		Role  string `json:"role"`
	}
	if err := app.api.JSON(
		c.Context(),
		"POST",
		target.base+"/invitations",
		p.CreateTeamInvitation{Email: email, Role: role},
		&invitation,
	); err != nil {
		return err
	}
	suffix := ""
	if target.show != "" {
		suffix = " to " + target.show
	}
	fmt.Printf("Invited %s%s as %s.\n", invitation.Email, suffix, invitation.Role)
	return nil
}

func (app *application) changeMemberRole(c *cobra.Command, _ []string) error {
	target, err := membershipTarget(c)
	if err != nil {
		return err
	}
	role, err := membershipRole(c)
	if err != nil {
		return err
	}
	member := flagString(c, "member")
	if err := requireID(member, "usr_"); err != nil {
		return err
	}
	if err := app.api.JSON(
		c.Context(),
		"PUT",
		target.base+"/members/"+member,
		p.UpdateTeamMemberRole{Role: role},
		nil,
	); err != nil {
		return err
	}
	fmt.Printf("Changed %s %s to %s.\n", target.label, member, role)
	return nil
}

func (app *application) removeMember(c *cobra.Command, _ []string) error {
	target, err := membershipTarget(c)
	if err != nil {
		return err
	}
	member := flagString(c, "member")
	if err := requireID(member, "usr_"); err != nil {
		return err
	}
	if err := mustConfirm(c); err != nil {
		return err
	}
	if err := app.api.JSON(
		c.Context(),
		"DELETE",
		target.base+"/members/"+member,
		nil,
		nil,
	); err != nil {
		return err
	}
	fmt.Printf("Removed %s %s.\n", target.label, member)
	return nil
}
