# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

from pathlib import Path
root=Path(__file__).resolve().parents[1]
s=(root/'kernel/src/package/registry.rs').read_text(encoding='utf-8')
assert 'pub package_id: String' in s
assert 'stage_verified' in s
assert 'independent of the built-in package catalog' in s

# The host-side window onto that registry is `interos.py list`. A query the
# device refused, or answered without a list, is not evidence that nothing is
# installed, so the command has to tell the two apart instead of printing zero.
cli=(root/'tools/interos.py').read_text(encoding='utf-8')
body=cli.split('def cmd_list(',1)[1].split('\n# =',1)[0]
assert 'if not data.get("success")' in body, 'a refused registry query is reported as a list'
assert 'data.get("apps", [])' not in body, 'a missing package list defaults to an empty one'
assert 'answered without a package list' in body
print('forkable package registry contract: PASS')
