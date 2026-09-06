# Yes Sessions visual contract

The retired Electron interface is the visual reference for the GPUI rewrite. The rewrite preserves its information architecture, density, spacing, colors, typography, and interaction states instead of introducing a new visual direction.

## Product character

- Compact macOS session workspace with a quiet, utilitarian appearance.
- Two-column layout: a dense 320 px session sidebar and a flexible conversation detail pane.
- Low-contrast white or near-black surfaces, thin borders, dark navy default accent, and restrained shadows.
- System sans-serif typography. Body copy is 15 px; compact metadata is 10 to 12 px; the application title is 16 px semibold.

## Layout invariants

- Default window: 1200 x 800 px. Minimum window: 900 x 600 px.
- Transparent hidden-inset title bar with a 40 px application header.
- Header left inset: 76 px for macOS traffic lights.
- Expanded sidebar: 320 px. Session rows are approximately 36 px and remain single-line.
- Detail header and conversation use 16 px padding.
- Settings dialog: 576 px maximum width and 85% maximum window height.

## Tokens

- Spacing: 4, 6, 8, 12, 16, and 24 px.
- Radius: 4 px for compact controls, 6 px for segmented controls, 8 px for cards and the settings dialog, pill only for badges and toggles.
- Light background/card: HSL 0 0% 100%.
- Light foreground: HSL 222.2 84% 4.9%.
- Light border: HSL 214.3 31.8% 91.4%.
- Light muted surface: HSL 210 40% 96.1%.
- Light muted foreground: HSL 215.4 16.3% 46.9%.
- Default light primary: HSL 222.2 47.4% 11.2%.
- Dark background/card: HSL 222.2 84% 4.9%.
- Dark foreground: HSL 210 40% 98%.
- Dark border/muted surface: HSL 217.2 32.6% 17.5%.
- Dark muted foreground: HSL 215 20.2% 65.1%.
- Default dark primary: HSL 210 40% 98%.

## Component rules

- Header controls are icon-only, 32 px for sidebar and 36 px for settings.
- Provider selection is one 36 px dropdown-style control with the provider icon and label.
- Date/directory selection is a compact 10 px segmented control.
- Selected sessions use the light primary surface plus a 2 px primary rail on the left.
- A conversation turn owns one 32 px avatar. User text sits in a muted 12 px-padded bubble; assistant content is cardless; tool activity is nested beneath the assistant response.
- Settings tabs use a bottom underline, not filled tab buttons. Settings content is divided into compact rows or selectable feature cards.

## Scope exception

The old splash screen and file-preview UI are intentionally omitted. Mermaid diagrams use the approved `gpui-wry` and Mermaid implementation.
