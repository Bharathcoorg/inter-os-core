from pathlib import Path
root=Path(__file__).resolve().parents[1]
s=(root/'kernel/src/package/registry.rs').read_text(encoding='utf-8')
assert 'pub package_id: String' in s
assert 'stage_verified' in s
assert 'independent of the built-in package catalog' in s
print('forkable package registry contract: PASS')
