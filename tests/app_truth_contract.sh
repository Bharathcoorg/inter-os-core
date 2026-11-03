#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

set -euo pipefail

grep -q 'THREADS: \[ConversationThread; 0\] = \[\]' apps/messages/src/main.rs
grep -q 'AGENDA: \[CalendarEvent; 0\] = \[\]' apps/calendar/src/main.rs
grep -q 'PLAYLIST: \[AudioTrack; 0\] = \[\]' apps/music/src/main.rs
grep -q 'CONTACTS_BOOK: \[Contact; 0\] = \[\]' apps/dialer/src/main.rs
grep -q 'BOOKMARKS: \[Bookmark; 0\] = \[\]' apps/browser/src/main.rs
grep -q 'PHOTOS: \[PhotoItem; 0\] = \[\]' apps/gallery/src/main.rs
grep -q 'ITEMS_ALL: \[VfsItem; 0\] = \[\]' apps/file_manager/src/main.rs
grep -q 'pub static BOOKMARKS: \[Bookmark; 0\] = \[\]' apps/browser/src/main.rs
! grep -q 'INTER_DMA_AUDIO_PCM_BLOCK' apps/music/src/main.rs
! grep -q 'Sovereign Symphony in C' apps/music/src/main.rs
! grep -q '4 apps . 2 tasks' apps/launcher/src/main.rs
! grep -q 'Mail' apps/launcher/src/main.rs
! grep -q 'Maps' apps/launcher/src/main.rs
! grep -q 'Contacts' apps/launcher/src/main.rs
! grep -q 'Wallpaper theme' apps/launcher/src/main.rs
! grep -q 'flashlight_on' apps/launcher/src/main.rs
! grep -q 'music.play_arpeggio();' apps/music/src/main.rs

echo "application truth contract: PASS"
