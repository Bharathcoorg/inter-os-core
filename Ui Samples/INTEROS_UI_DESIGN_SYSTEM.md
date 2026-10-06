# InterOS Core UI/UX — Canonical Native UI Design System

## Scope

This document defines the **native InterOS visual and interaction language only**.

Android compatibility is a separate technical subsystem and is intentionally **not part of this UI specification**.

## Global design language

Minimal, calm, premium, functional, contextual, private, fast, open-source, low visual noise.

InterOS must feel like a complete consumer mobile operating system, not a desktop UI and not an Android/iOS skin.

## Core colors

- Background: `#F7F6F2`
- Surface: `#FFFFFF`
- Soft surface: `#EFEEE9`
- Border: `#E1E0DB`
- Primary text: `#111214`
- Secondary text: `#777773`
- Dark background: `#111214`
- Dark surface: `#25282A`
- Semantic colors should be restrained: muted green, blue, amber and warm red.

The overall UI should be predominantly neutral. Avoid rainbow-like interfaces.

## Typography

Use a clean modern system sans / Inter-like family.

- Display: 48–56 px
- H1: 36–44 px
- H2: 28–32 px
- H3: 20–24 px
- Body: 15–18 px
- Caption: 12–14 px
- Minimum touch target: 44×44 px
- Preferred touch target: 48×48 px

Typography should feel calm rather than oversized or decorative.

## Spacing

Use an 8-point base grid:

`4, 8, 12, 16, 20, 24, 32, 40, 48, 64`

Typical screen margin: 20–24 px.

## Radius

- Small/image: 8 px
- Controls/keypad: 12–14 px
- Standard cards: 16–20 px
- Sheets: 24 px
- Major containers/screens: 28–32 px
- Pills: 999 px

Do not turn every element into a card.

## Icons

Use one coherent icon family.

- Rounded line geometry
- Approximately 2 px stroke
- Rounded caps and joins
- 24×24 base grid
- Mostly monochrome
- Semantic color only when useful

Do not mix unrelated icon families.

## Materials

Four main surfaces:

1. Base — `#F7F6F2`
2. Elevated — `#FFFFFF`
3. Soft — `#EFEEE9`
4. Dark — `#111214`

Translucent/blurred surfaces may be used for Activities and overlays, but glassmorphism must not be used everywhere.

## Motion

- Press feedback: 80–120 ms
- Standard transition: 180–240 ms
- Major transition: 250–350 ms
- Activity expansion: 250–300 ms
- Quick controls: 120–180 ms

Motion should be fast, subtle and deliberate. Avoid excessive bounce, decorative animations or unnecessary parallax.

# Native InterOS screens

## 1. Lock Screen

Atmospheric photographic background, large calm clock, date, minimal information and two bottom shortcuts.

No notification clutter.

## 2. Home Surface

Contextual greeting, weather, Today card and six primary actions.

The Home Surface is not a traditional icon-only Android launcher.

## 3. App View

Search field, category pills and clean 3-column application grid.

Categories:

- All
- Work
- Personal
- Tools

## 4. Activity Space

A defining InterOS concept.

Activities are contextual workspaces such as:

- Work
- Personal
- Research
- Media

Each Activity can show its apps, tasks, files and current context.

Activity Space should feel different from a conventional recent-apps screen.

## 5. Events

A unified chronological event surface.

Can contain:

- Messages
- System events
- App events
- Downloads
- Updates
- Device events

Events are grouped by Today / Yesterday etc.

## 6. System Space

Native InterOS settings architecture.

Use semantic categories:

- Device
- Network
- Personal
- Apps
- Power
- Security
- System

Do not blindly reproduce conventional Android Settings.

## 7. Quick Controls

Fast one-handed system controls.

Primary controls:

- Wi-Fi
- Bluetooth
- Mobile
- Airplane

Secondary controls:

- Brightness
- Volume
- Torch
- Auto Rotate
- Dark Mode

## 8. Power Menu

Extremely simple and high-certainty:

- Lock device
- Restart
- Power off
- Emergency

Emergency receives restrained red/orange treatment.

## 9. Phone

Simple dialer with:

- Dial
- Recents
- Contacts
- Large keypad
- Clear call action

## 10. Messages

Clean conversation list with avatars, names, previews, timestamps and unread state.

## 11. Files

Simple file categories:

- Recent
- Images
- Videos
- Documents
- Downloads
- Internal Storage
- Cloud (optional)

## 12. Gallery

Image-first 3-column grid with:

- All
- Albums
- Favorites

Bottom navigation can include Photos / Albums / Places.

## 13. Browser

Native InterOS browser surface:

- Search/address field
- Quick destinations
- Discover/editorial content

Do not visually copy Chrome.

## 14. Media Player

Immersive dark surface:

- Artwork
- Track
- Artist
- Progress
- Previous
- Play/Pause
- Next
- Secondary controls

## 15. Camera

Dark preview-first UI.

Top controls, mode selector, large shutter and camera-switch control.

Modes:

- Photo
- Video
- Portrait

## 16. Calculator

Minimal calculator with large right-aligned display and soft keypad.

Equals button uses the primary dark treatment.

# Navigation model

The supplied reference defines the following native InterOS gestures:

- Swipe up → Activity Space
- Swipe from left edge → Back
- Swipe from right edge → Quick Controls
- Tap system/top area → System Space
- Long press → Context actions
- Pinch → Activity management

These gestures should be treated as a coherent system rather than isolated shortcuts.

# Design principles

1. InterOS must have its own visual identity.
2. Native UI comes first.
3. Minimalism must not reduce usability.
4. Every screen should have a clear hierarchy.
5. Use cards only when they communicate grouping/context.
6. Prefer whitespace over visual decoration.
7. Prefer semantic icons over text-heavy controls where appropriate.
8. Keep interactions predictable.
9. Keep animations fast and restrained.
10. Light and dark themes must both feel intentionally designed.
11. The UI must scale cleanly across phone sizes.
12. Accessibility and touch targets are first-class requirements.
13. Do not copy Android or iOS UI patterns merely because they are familiar.
14. Activities, Events and System Space are core InterOS concepts and should feel native to the OS.

# Reference assets

The accompanying PNG files in this package are visual reference screens. They establish composition, hierarchy, spacing, density and visual direction. They are not a requirement to reproduce every generated pixel literally.

Implementation should create reusable native components and tokens rather than hard-coding individual screens.
