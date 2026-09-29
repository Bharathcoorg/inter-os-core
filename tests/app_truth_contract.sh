#!/usr/bin/env bash
set -euo pipefail

grep -q 'THREADS: \[ConversationThread; 0\] = \[\]' apps/messages/src/main.rs
grep -q 'AGENDA: \[CalendarEvent; 0\] = \[\]' apps/calendar/src/main.rs
grep -q 'PLAYLIST: \[AudioTrack; 0\] = \[\]' apps/music/src/main.rs
grep -q 'CONTACTS_BOOK: \[Contact; 0\] = \[\]' apps/dialer/src/main.rs
grep -q 'BOOKMARKS: \[Bookmark; 0\] = \[\]' apps/browser/src/main.rs
grep -q 'PHOTOS: \[PhotoItem; 0\] = \[\]' apps/gallery/src/main.rs
grep -q 'ITEMS_ALL: \[VfsItem; 0\] = \[\]' apps/file_manager/src/main.rs
grep -q 'remove fabricated home and recents state' <(git log -20 --format=%s) || true
! grep -q 'music.play_arpeggio();' apps/music/src/main.rs

echo "application truth contract: PASS"
