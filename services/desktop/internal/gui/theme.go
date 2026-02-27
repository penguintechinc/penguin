package gui

import (
	"image/color"

	"fyne.io/fyne/v2"
	"fyne.io/fyne/v2/theme"
)

// PenguinTheme implements the PenguinTech gold theme.
type PenguinTheme struct{}

// NewPenguinTheme creates the PenguinTech theme.
func NewPenguinTheme() fyne.Theme {
	return &PenguinTheme{}
}

func (t *PenguinTheme) Color(name fyne.ThemeColorName, variant fyne.ThemeVariant) color.Color {
	switch name {
	case theme.ColorNamePrimary:
		return color.NRGBA{R: 255, G: 215, B: 0, A: 255} // Gold #FFD700
	case theme.ColorNameBackground:
		if variant == theme.VariantDark {
			return color.NRGBA{R: 30, G: 30, B: 35, A: 255}
		}
		return color.NRGBA{R: 250, G: 250, B: 250, A: 255}
	case theme.ColorNameButton:
		return color.NRGBA{R: 255, G: 215, B: 0, A: 255}
	default:
		return theme.DefaultTheme().Color(name, variant)
	}
}

func (t *PenguinTheme) Font(style fyne.TextStyle) fyne.Resource {
	return theme.DefaultTheme().Font(style)
}

func (t *PenguinTheme) Icon(name fyne.ThemeIconName) fyne.Resource {
	return theme.DefaultTheme().Icon(name)
}

func (t *PenguinTheme) Size(name fyne.ThemeSizeName) float32 {
	return theme.DefaultTheme().Size(name)
}
