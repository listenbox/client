//nolint:mnd // The design system's values live here and nowhere else.
package main

import "github.com/egoist/mygo/ui"

// The desktop's design system is the monorepo's DESIGN.md, projected onto
// MyGo as its guide to styling suggests: the root's roles go on a copy of
// the window's theme, so built-in widgets take them and the theme still
// follows the desktop's contrast and text size, and custom elements read
// colors from palette and sizes from the theme (Space, Rem). View code never
// writes a color, a font size, a radius or a shadow of its own.

// The root DESIGN.md's colors, in its own hex values.
var (
	paper       = ui.Hex("#f5f4f0")
	paperDeep   = ui.Hex("#ebe9e3")
	white       = ui.Hex("#ffffff")
	inkColor    = ui.Hex("#0a0b0f")
	inkRaised   = ui.Hex("#15161c")
	lineBase    = ui.Hex("#111217") // line: rgb(17 18 23 / 0.08)
	graphite    = ui.Hex("#1c1d20") // fg
	slate       = ui.Hex("#5b5e69") // fg-soft
	snow        = ui.Hex("#f3f4f7")
	snowSoft    = ui.Hex("#a5a8b6")
	accent      = ui.Hex("#5165ee")
	accentHover = ui.Hex("#4054e0")
	accentLight = ui.Hex("#8f9cff")
	mist        = ui.Hex("#eef0fd")
	lavender    = ui.Hex("#e7e4fc")
	inkMist     = ui.Hex("#1f2235") // the Dashboard's mist on ink
	ok          = ui.Hex("#377d40")
	okOnInk     = ui.Hex("#7fd08a")
	danger      = ui.Hex("#d4483f")
	dangerOnInk = ui.Hex("#ff8a80")
)

// The root's translucent tones: line on paper, ink-line and ink-tile on ink.
const (
	lineAlpha    = 0.08
	inkLineAlpha = 0.09
	inkTileAlpha = 0.05
	// selectionAlpha tints selected text on ink.
	selectionAlpha = 0.32
	// edgeAlpha is the Dashboard's control hairline: graphite or snow at 12%.
	edgeAlpha = 0.12
	// dangerTextMix darkens the root's danger toward graphite until it reads
	// at 4.5:1 as text on paper, white and its own hover tint; the root's
	// danger alone is 4.0:1 on paper and stays for edges.
	dangerTextMix = 0.2
)

// palette holds the root DESIGN.md's color roles for one tone: paper in the
// light appearance, ink in the dark. Roles the MyGo theme lacks live only
// here. Every text-carrying role has its ink counterpart (the root's
// Two-Tone Rule), and the dark appearance swaps the whole set.
type palette struct {
	// background is the window; sheet the white panels, popovers and the
	// quit notice; rail the sidebar, a white panel beside the paper.
	background, sheet, rail ui.Color
	// control is the face of quiet actions, fields and the team switcher,
	// controlHover their hovered and pressed face.
	control, controlHover ui.Color
	// fg and fgSoft are primary and secondary text.
	fg, fgSoft ui.Color
	// edge bounds controls at 3:1 or more against what they sit on; line is
	// the root's quiet hairline for dividers and panel edges.
	edge, line ui.Color
	// selected marks the chosen podcast and selectedText its title, as the
	// Dashboard's current nav item; placeholder fills missing artwork; track
	// is a progress bar's empty part.
	selected, selectedText, placeholder, track ui.Color
	// button is the quiet action's face: white on paper, as the Dashboard's
	// light button.
	button ui.Color
	// action fills primary actions, actionText the text on it; focus rings
	// the keyboard focus. As in the Dashboard, periwinkle marks primary
	// actions, focus, the selected podcast and progress, and nothing else.
	action, actionHover, actionText, focus ui.Color
	// badge is a paid plan's solid pill, badgeSoft and badgeSoftText the
	// Free plan's quiet one.
	badge, badgeText, badgeSoft, badgeSoftText ui.Color
	// danger edges failures, dangerText is their message; success is what
	// went well.
	danger, dangerText, success ui.Color
	// selection highlights selected text.
	selection ui.Color
}

var lightPalette = palette{
	background:    paper,
	sheet:         white,
	rail:          white,
	control:       paper,     // button-quiet, the paper field
	controlHover:  paperDeep, // button-quiet-hover
	fg:            graphite,
	fgSoft:        slate,
	edge:          graphite.Alpha(edgeAlpha),
	line:          lineBase.Alpha(lineAlpha),
	selected:      mist,
	selectedText:  accentHover,
	placeholder:   paperDeep, // skeletons
	track:         paperDeep, // empty progress tracks
	button:        white,
	action:        accent,
	actionHover:   accentHover,
	actionText:    white,
	focus:         accent,
	badge:         accent,
	badgeText:     white,
	badgeSoft:     mist,
	badgeSoftText: accentHover,
	danger:        danger,
	dangerText:    danger.Mix(graphite, dangerTextMix),
	success:       ok,
	selection:     lavender,
}

var darkPalette = palette{
	background:    inkColor,
	sheet:         inkRaised,
	rail:          inkRaised,
	control:       snow.Alpha(inkTileAlpha), // ink-tile
	controlHover:  snow.Alpha(inkLineAlpha),
	fg:            snow,
	fgSoft:        snowSoft,
	edge:          snow.Alpha(edgeAlpha),
	line:          snow.Alpha(inkLineAlpha), // ink-line
	selected:      inkMist,
	selectedText:  accentLight,
	placeholder:   snow.Alpha(inkLineAlpha),
	track:         snow.Alpha(inkLineAlpha),
	button:        inkRaised,
	action:        accent, // white text holds 4.7:1 only on the full accent
	actionHover:   accentHover,
	actionText:    white,
	focus:         accentLight,
	badge:         accent,
	badgeText:     white,
	badgeSoft:     inkMist,
	badgeSoftText: accentLight,
	danger:        dangerOnInk,
	dangerText:    dangerOnInk,
	success:       okOnInk,
	selection:     accentLight.Alpha(selectionAlpha),
}

// colors returns the palette of the appearance being built.
func colors(c *ui.Context) *palette {
	if c.Theme().Dark {
		return &darkPalette
	}
	return &lightPalette
}

// design is what view code styles with: the palette of the appearance being
// built and the spacing scale (xs 4 … xxl 32) in the theme's units.
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

// The root's radius scale, in DIPs, stepped by size.
const (
	radiusItem  = 10  // menu items: the nav's hover tiles
	radiusSmall = 12  // small artwork and the badge's inner chip
	radiusCtl   = 14  // every control
	radiusTile  = 18  // podcast rows, panels, notices and popovers
	radiusSheet = 22  // the quit notice
	radiusCover = 24  // large artwork
	radiusCard  = 28  // the sidebar panel and content cards, as the Dashboard's
	radiusPill  = 999 // badges only, never controls

	// controlRadius rounds every control alike: buttons, fields and the team
	// switcher. No control is a capsule.
	controlRadius = radiusCtl
)

// typeRole is one of the root DESIGN.md's type roles: its size in the root's
// rem (16px), its weight and its tracking in em.
type typeRole struct {
	rem      float32
	weight   int
	tracking float32
}

// The theme's FontSize is the root's small control size (0.875rem, 14px), so
// root sizes become theme rems by this ratio and follow the desktop's text
// size as MyGo's widgets do.
const rootControlRem = 0.875

var (
	typeDisplay      = typeRole{3, 700, -0.03}    // the quit shortcut
	typePageTitle    = typeRole{2.5, 700, -0.03}  // the Dashboard's h1 at desktop widths
	typeTitle        = typeRole{1.25, 700, -0.02} // panel title
	typeFeatureTitle = typeRole{1.0625, 600, 0}   // label, largest
	typeField        = typeRole{0.875, 400, 0}    // secondary body
	typeLabel        = typeRole{0.875, 600, 0}    // label, smallest
	typeButton       = typeRole{0.875, 600, 0}    // small button
	typeBadge        = typeRole{0.6875, 600, 0}   // tag
	typeBadgeDetail  = typeRole{0.625, 700, 0.02} // the badge's "4K"
)

// set gives text its role's size, weight and tracking.
func (r typeRole) set(c *ui.Context, text ui.Element) ui.Element {
	size := c.Theme().Rem(r.rem / rootControlRem)
	text.FontSize(size).FontWeight(r.weight)
	if r.tracking != 0 {
		text.LetterSpacing(size * r.tracking)
	}
	return text
}

// The root's shadow vocabulary: long, soft and tinted, never hard.
var (
	floatShadow = inkColor.Alpha(0.28)       // 0 24px 48px -18px
	softShadow  = ui.RGBA(23, 26, 56, 0.18)  // 0 12px 32px -14px
	accentGlow  = ui.RGBA(81, 101, 238, 0.9) // 0 10px 24px -12px, under accent actions
)

// overlay gives a layered surface, a popover or the quit notice, the root's
// float shadow.
func overlay(e ui.Element) ui.Element {
	return e.Shadow(0, 24, 48, -18, floatShadow)
}

// soft gives an element the root's soft shadow.
func soft(e ui.Element) ui.Element {
	return e.Shadow(0, 12, 32, -14, softShadow)
}

// panel is the Dashboard's white card on the paper window: 28-DIP corners and
// the soft shadow; on ink, an ink-raised card with the ink-line hairline.
func panel(c *ui.Context, e ui.Element) ui.Element {
	d := tokens(c)
	e = soft(e.Radius(radiusCard).Background(d.sheet))
	if c.Theme().Dark {
		e = e.Border(1, d.line)
	}
	return e
}

// card is a panel holding one section of a page: cardPad inside, its parts
// a step apart, as the Dashboard's form panels.
func card(c *ui.Context, content func()) {
	panel(c, ui.Column(c).FillWidth().Padding(cardPad).Gap(tokens(c).lg)).Children(content)
}

// Component metrics shared with the Dashboard and root components.
const (
	controlHeight      = 44  // every control: the root's md button, fields and the team switcher
	controlInset       = 20  // a button's sides, the root's md button
	fieldInset         = 16  // a field's sides
	cardPad            = 24  // the root's card-pad floor, inside every 28-DIP card
	railPad            = 20  // the Dashboard panel's padding floor, inside the sidebar
	gridGap            = 24  // between cards
	pageInset          = 40  // the content pane's margin, the root's gutter at desktop widths
	focusRingOffset    = 3   // the root's 2px outline, offset 3px
	menuWidth          = 288 // the Dashboard's dropdown (w-72)
	menuItemHeight     = 36
	menuListHeight     = 216 // six items before the team list scrolls
	frame              = 10  // the paper frame around the inset sidebar
	coverSmall         = 44
	coverLarge         = 120
	hudWidth           = 280
	readableWidth      = 420
	iconSmall          = 14
	iconMedium         = 16
	iconMark           = 18 // a destination mark leading a handoff action
	minimumSidebar     = 280
	maximumSidebar     = 480
	minimumWorkspace   = 360
	cookieEditorHeight = 160
	titleLines         = 3   // page titles wrap to three lines, list titles to two
	placeholderIcon    = 0.4 // missing artwork's headphones, of the cover's size
	hoverTint          = 0.1 // a tinted menu item's hover, of its tint
	badgeHeight        = 22
	badgeDetailFill    = 0.2 // Video 4K's resolution chip, of its text
	controlFontSize    = 14  // the root's small control text, 0.875rem of 16px
)

// applyTheme puts the root's roles and metrics on a copy of the window's
// theme, which keeps following the desktop's contrast and text size.
func applyTheme(c *ui.Context) {
	p := colors(c)
	theme := *c.Theme()
	theme.FontSize = controlFontSize
	theme.Radius = controlRadius
	theme.Background, theme.Surface = p.background, p.button
	theme.SurfaceHover, theme.SurfacePressed = p.controlHover, p.controlHover
	theme.Text, theme.TextMuted, theme.Border = p.fg, p.fgSoft, p.edge
	theme.Accent, theme.AccentHover, theme.AccentPressed = p.action, p.actionHover, p.actionHover
	theme.AccentText, theme.InactiveAccent = p.actionText, p.action
	theme.Danger, theme.Success = p.danger, p.success
	theme.Focus, theme.FocusRingOffset = p.focus, focusRingOffset
	theme.Selection = p.selection
	c.SetTheme(&theme)
}

// Every control shares one shape, the root's md button: controlHeight tall,
// controlRadius round, its content inset by controlInset (a field's by
// fieldInset), so controls stacked or set side by side line up. View code
// never sizes or rounds a control itself.

// primaryButton and button are the filled and quiet actions, in the
// button type role. An available accent action carries the root's accent
// shadow; primaryAction builds one that may be disabled, without the glow,
// as the root's disabled button has none (MyGo stacks shadows, so one cannot
// be taken back once set).
func primaryButton(c *ui.Context, label string) ui.Element {
	return primaryAction(c, label, false)
}

func primaryAction(c *ui.Context, label string, disabled bool) ui.Element {
	e := control(c, ui.PrimaryButton(c, label)).FontWeight(typeButton.weight).Disabled(disabled)
	if !disabled {
		e = e.Shadow(0, 10, 24, -12, accentGlow)
	}
	return e
}

func button(c *ui.Context, label string) ui.Element {
	return control(c, ui.Button(c, label)).FontWeight(typeButton.weight)
}

// handoffButton is a quiet action that leaves the app, led by the mark of
// where it goes: YouTube in its platform colors, Listenbox as its app icon.
func handoffButton(c *ui.Context, label string, mark *ui.SVG) ui.Element {
	d := tokens(c)
	e := button(c, "").Label(label).Gap(d.sm)
	e.Children(func() {
		ui.Image(c, mark).Size(iconMark, iconMark).Shrink(0)
		ui.Text(c, label).SingleLine()
	})
	return e
}

// textField is the Dashboard's field: a paper recess.
func textField(c *ui.Context, value *string) ui.Element {
	return control(c, ui.TextInput(c, value)).Background(tokens(c).control).Padding(0, fieldInset)
}

// control gives an element the shared control shape.
func control(_ *ui.Context, e ui.Element) ui.Element {
	return e.Height(controlHeight).Radius(controlRadius).Padding(0, controlInset)
}
