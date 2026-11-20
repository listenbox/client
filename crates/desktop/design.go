//nolint:mnd // The design system's values live here and nowhere else.
package main

import "github.com/egoist/mygo/ui"

// The desktop's design system is the monorepo's DESIGN.md, projected onto
// MyGo as its guide to styling suggests: the root's roles go on a copy of
// the window's theme, so built-in widgets take them and the theme still
// follows the desktop's contrast and text size, and custom elements read
// colors from palette and sizes from the theme (Space, Rem). View code never
// writes a color, a font size, a radius or a shadow of its own.

// palette holds the root DESIGN.md's color roles for one appearance, in its
// own OKLCH values. Roles the MyGo theme lacks live only here.
type palette struct {
	background, sheet, surfaceHover, rail, railControl, railControlHover,
	ink, muted, border, divider, selected,
	action, actionHover, actionInk, focus,
	primarySoft, accentStrong, railPlan, danger, success, warning ui.Color
}

//nolint:dupl // The light and dark palettes are parallel token tables.
var lightPalette = palette{
	background:       ui.Oklch(0.9851, 0, 0), // bg
	sheet:            ui.Oklch(1, 0, 0),
	surfaceHover:     ui.Oklch(0.9401, 0, 0),
	rail:             ui.Oklch(0.9612, 0, 0),
	railControl:      ui.Oklch(1, 0, 0),
	railControlHover: ui.Oklch(0.9401, 0, 0),
	ink:              ui.Oklch(0.2435, 0, 0),
	muted:            ui.Oklch(0.4532, 0, 0),
	border:           ui.Oklch(0.8761, 0, 0),
	divider:          ui.Oklch(0.9219, 0, 0),
	selected:         ui.Oklch(0.9219, 0, 0), // navigation-active
	action:           ui.Oklch(0.3092, 0, 0), // primary
	actionHover:      ui.Oklch(0.2264, 0, 0),
	actionInk:        ui.Oklch(1, 0, 0),
	focus:            ui.Oklch(0.3092, 0, 0),
	primarySoft:      ui.Oklch(0.9642, 0, 0),
	accentStrong:     ui.Oklch(0.3092, 0, 0),
	railPlan:         ui.Oklch(0.3092, 0, 0),
	danger:           ui.Oklch(0.5861, 0.1563, 27.2),
	success:          ui.Oklch(0.5368, 0.0853, 140.3),
	warning:          ui.Oklch(0.726, 0.1421, 78.7),
}

//nolint:dupl // The light and dark palettes are parallel token tables.
var darkPalette = palette{
	background:       ui.Oklch(0.2134, 0, 0),
	sheet:            ui.Oklch(0.252, 0, 0),
	surfaceHover:     ui.Oklch(0.329, 0, 0),
	rail:             ui.Oklch(0.2478, 0, 0),
	railControl:      ui.Oklch(0.2931, 0, 0),
	railControlHover: ui.Oklch(0.329, 0, 0),
	ink:              ui.Oklch(0.9702, 0, 0),
	muted:            ui.Oklch(0.8015, 0, 0),
	border:           ui.Oklch(0.4349, 0, 0),
	divider:          ui.Oklch(0.3446, 0, 0),
	selected:         ui.Oklch(0.3368, 0, 0),
	action:           ui.Oklch(0.4532, 0, 0),
	actionHover:      ui.Oklch(0.5173, 0, 0),
	actionInk:        ui.Oklch(1, 0, 0),
	focus:            ui.Oklch(0.8545, 0, 0),
	primarySoft:      ui.Oklch(0.3092, 0, 0),
	accentStrong:     ui.Oklch(0.8545, 0, 0),
	railPlan:         ui.Oklch(0.4532, 0, 0),
	danger:           ui.Oklch(0.68, 0.18, 25), // the Dashboard's dark danger
	success:          ui.Oklch(0.5368, 0.0853, 140.3),
	warning:          ui.Oklch(0.726, 0.1421, 78.7),
}

// colors returns the palette of the appearance being built.
func colors(c *ui.Context) *palette {
	if c.Theme().Dark {
		return &darkPalette
	}
	return &lightPalette
}

// design is what view code styles with: the palette of the appearance being
// built and the root's spacing scale (xs 4 … xxl 32) in the theme's units.
type design struct {
	*palette

	xs, sm, md, lg, xl, xxl float32
}

func tokens(c *ui.Context) design {
	t := c.Theme()
	return design{
		palette: colors(c),
		xs:      t.Space(1),
		sm:      t.Space(2),
		md:      t.Space(3),
		lg:      t.Space(4),
		xl:      t.Space(6),
		xxl:     t.Space(8),
	}
}

// The root's radius scale, in DIPs.
const (
	radiusXS      = 6   // micro-controls: menu items, chips inside badges
	radiusSM      = 10  // rows and small artwork
	radiusMD      = 14  // fields and icon tiles
	radiusSurface = 16  // panels, popovers and cards
	radiusMedia   = 20  // large artwork and overlays
	radiusPill    = 999 // chips and badges only, never controls

	// controlRadius rounds every control alike: buttons, fields and the team
	// switcher, so a capsule never sits beside a rounded rectangle.
	controlRadius = radiusMD
)

// typeRole is one of the root DESIGN.md's type roles: its size in the root's
// rem (16px) and its weight.
type typeRole struct {
	rem    float32
	weight int
}

// The theme's FontSize is the root's control size (0.875rem, 14px), so
// root sizes become theme rems by this ratio and follow the desktop's text
// size as MyGo's widgets do.
const rootControlRem = 0.875

var (
	typeDisplay      = typeRole{3, 700}
	typePageTitle    = typeRole{1.875, 700}
	typeTitle        = typeRole{1.125, 650}
	typeFeatureTitle = typeRole{1.0625, 700}
	typeField        = typeRole{0.875, 400}
	typeLabel        = typeRole{0.875, 600}
	typeButton       = typeRole{0.875, 700}
	typeBadge        = typeRole{0.75, 700} // the Dashboard's plan badge
	typeBadgeDetail  = typeRole{0.6875, 900}
)

// set gives text its role's size and weight.
func (r typeRole) set(c *ui.Context, text ui.Element) ui.Element {
	return text.FontSize(c.Theme().Rem(r.rem / rootControlRem)).FontWeight(r.weight)
}

// overlay gives a layered surface the root's overlay elevation.
func overlay(e ui.Element) ui.Element {
	return e.Shadow(0, 8, 24, 0, ui.Oklch(0, 0, 0).Alpha(0.14))
}

// Component metrics shared with the Dashboard and root components.
const (
	controlHeight      = 36  // every control: buttons, fields and the team switcher
	menuWidth          = 288 // the Dashboard's dropdown (w-72)
	menuItemHeight     = 36
	menuListHeight     = 216 // six items before the team list scrolls
	coverSmall         = 44
	coverLarge         = 120
	hudWidth           = 280
	readableWidth      = 420
	iconSmall          = 14
	iconMedium         = 16
	minimumSidebar     = 240
	maximumSidebar     = 480
	minimumWorkspace   = 360
	cookieEditorHeight = 160
	titleLines         = 3    // page titles wrap to three lines, list titles to two
	placeholderIcon    = 0.4  // missing artwork's headphones, of the cover's size
	dangerTint         = 0.08 // a danger notice's fill, of the danger color
	hoverTint          = 0.1  // a tinted menu item's hover, of its tint
	badgeHeight        = 22   // the Dashboard's plan badge (h-5.5)
	badgeRing          = 0.15 // a paid badge's inner ring, of its ink
	badgeOutline       = 0.2  // the Free badge's outline, of its ink
	badgeDetailFill    = 0.12 // Video 4K's resolution chip, of its ink
	controlFontSize    = 14   // the root's control text, 0.875rem of 16px
)

// applyTheme puts the root's roles and metrics on a copy of the window's
// theme, which keeps following the desktop's contrast and text size.
func applyTheme(c *ui.Context) {
	p := colors(c)
	theme := *c.Theme()
	theme.FontSize = controlFontSize
	theme.Radius = controlRadius
	theme.Background, theme.Surface = p.background, p.sheet
	theme.SurfaceHover, theme.SurfacePressed = p.surfaceHover, p.selected
	theme.Text, theme.TextMuted, theme.Border = p.ink, p.muted, p.border
	theme.Accent, theme.AccentHover, theme.AccentPressed = p.action, p.actionHover, p.actionHover
	theme.AccentText, theme.InactiveAccent = p.actionInk, p.action
	theme.Danger, theme.Success, theme.Warning = p.danger, p.success, p.warning
	theme.Focus = p.focus
	c.SetTheme(&theme)
}

// Every control shares one shape: controlHeight tall, controlRadius round,
// its content inset by the root's 12 DIPs, so controls stacked or set side by
// side line up. View code never sizes or rounds a control itself.

// primaryButton and button are the root's actions, in the button type role.
func primaryButton(c *ui.Context, label string) ui.Element {
	return control(c, ui.PrimaryButton(c, label)).FontWeight(typeButton.weight)
}

func button(c *ui.Context, label string) ui.Element {
	return control(c, ui.Button(c, label)).FontWeight(typeButton.weight)
}

// textField is the root's input.
func textField(c *ui.Context, value *string) ui.Element {
	return control(c, ui.TextInput(c, value))
}

// control gives an element the shared control shape.
func control(c *ui.Context, e ui.Element) ui.Element {
	return e.Height(controlHeight).Radius(controlRadius).Padding(0, tokens(c).md)
}
