#!/usr/bin/env python3
"""Splits the launcher's per-pixel screen renderers into one file per screen.

WHY: every screen is a function in one 3,749-line file, so two agents editing
different screens collide on every save. One file per screen makes the work
genuinely parallel. Bin-crate submodules can see the crate root's private items,
so no visibility changes or library reshuffle is needed.

The scripts are verified with real boots and screendumps afterwards; this only
relocates code, it does not change behaviour.
"""
import os
import re
import sys

SRC = "apps/launcher/src/main.rs"
SCREENS_DIR = "apps/launcher/src/screens"

# (function name, module file, doc summary)
SCREENS = [
    ("get_home_color", "home.rs", "Home surface: launcher deck, dock and status card."),
    ("get_activity_color", "activity.rs", "Activity Space: the multitasking overview deck."),
    ("get_quick_controls_color", "quick_controls.rs", "Quick Controls shade: radios, sliders and toggles."),
    ("get_system_space_color", "system_space.rs", "System Space: device and account settings."),
    ("get_lock_screen_color", "lock.rs", "Lock Screen: wallpaper, clock, controls and unlock affordances."),
    ("get_power_menu_color", "power.rs", "Power Menu: lock, restart and power off."),
    ("get_app_drawer_color", "app_drawer.rs", "App Drawer: the full catalogue grid."),
    ("get_notifications_color", "notifications.rs", "Notifications shade."),
    ("get_live_widgets_color", "live_widgets.rs", "Live Widgets dashboard."),
]


def read():
    with open(SRC, encoding="utf-8") as fh:
        return fh.read()


def split_functions(lines):
    """Returns (header, [(name, start, end)]) by locating top-level fn bodies."""
    starts = []
    for i, line in enumerate(lines):
        m = re.match(r"(?:#\[inline\(never\)\]\n)?fn (\w+)\(", line)
        if m:
            starts.append((m.group(1), i))
    # find where each fn body ends by brace matching from its signature
    spans = {}
    for idx, (name, start) in enumerate(starts):
        depth = 0
        began = False
        end = None
        j = start
        while j < len(lines):
            depth += lines[j].count("{") - lines[j].count("}")
            if "{" in lines[j]:
                began = True
            if began and depth <= 0:
                end = j
                break
            j += 1
        if end is not None:
            spans[name] = (start, end)
    return spans


def main():
    lines = read().split("\n")
    spans = split_functions(lines)
    missing = [n for n, _, _ in SCREENS if n not in spans]
    if missing:
        print("could not locate:", missing)
        return 1

    os.makedirs(SCREENS_DIR, exist_ok=True)

    # Remove the moved functions from main.rs, last first so indices stay valid.
    kept = lines[:]
    for name, _, _ in sorted(SCREENS, key=lambda s: spans[s[0]][0], reverse=True):
        start, end = spans[name]
        del kept[start:end + 1]

    header = kept[:kept.index("") if "" in kept else len(kept)]
    # Insert `mod screens;` after the existing use lines.
    insert_at = 0
    for i, line in enumerate(kept[:40]):
        if line.startswith("use ") or line.startswith("mod "):
            insert_at = i + 1
    kept[insert_at:insert_at] = [
        "",
        "// WHY one file per screen: every renderer was a function in this file, so two",
        "// people editing different screens collided on every save. Submodules can reach",
        "// the crate root's private items, so no visibility change was needed.",
        "mod screens;",
    ]

    with open(SRC, "w", encoding="utf-8", newline="\n") as fh:
        fh.write("\n".join(kept))

    for name, filename, doc in SCREENS:
        start, end = spans[name]
        body = lines[start:end + 1]
        # Publish it so main.rs can dispatch to it.
        body = [body[0].replace("fn ", "pub fn ", 1)] + body[1:]
        body = ["use crate::{test_text, LauncherState};", ""] + body
        with open(os.path.join(SCREENS_DIR, filename), "w",
                  encoding="utf-8", newline="\n") as fh:
            fh.write("//! {}\n".format(doc))
            fh.write("//!\n")
            fh.write("//! WHY `pub`: `render_view` in the crate root dispatches here by view\n")
            fh.write("//! mode. Geometry and colour come from `crate::theme` and\n")
            fh.write("//! `crate::layout`, never from literals local to this file.\n\n")
            fh.write("#![allow(unused_imports)]\n\n")
            fh.write("\n".join(body))
            fh.write("\n")

    with open(os.path.join(SCREENS_DIR, "mod.rs"), "w",
              encoding="utf-8", newline="\n") as fh:
        fh.write("//! One module per screen, so screens can be edited in parallel.\n\n")
        for _, filename, doc in SCREENS:
            fh.write("pub mod {};\n".format(filename[:-3]))
        fh.write("\npub use {}::*;\n".format(", ".join(f[:-3] for f, _, _ in SCREENS)))

    print("split {} screens into {}".format(len(SCREENS), SCREENS_DIR))
    return 0


if __name__ == "__main__":
    sys.exit(main())