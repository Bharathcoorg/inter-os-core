# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

from pathlib import Path
r=Path(__file__).resolve().parents[1]/'kernel/src/package/rapp.rs'
d=Path(__file__).resolve().parents[1]/'kernel/src/package/dependency.rs'
rs=r.read_text(encoding='utf-8'); ds=d.read_text(encoding='utf-8')
assert 'pub policy:' in rs and 'from_rapp_capabilities' in rs
assert 'from_rapp_capabilities' in ds
print('RAPP permission policy integration contract: PASS')
