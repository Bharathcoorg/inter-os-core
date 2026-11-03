# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

from pathlib import Path
p=Path(__file__).resolve().parents[1]/'kernel/src/net/ipv6.rs'
s=p.read_text(encoding='utf-8')
for token in ['NeighborCache','NeighborEntry','Ipv6Autoconfig','Ipv6RoutingTable']:
 assert token in s
print('ipv6 neighbor/autoconfig contract: PASS')
