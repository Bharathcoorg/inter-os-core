# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

from pathlib import Path

ROOT=Path(__file__).resolve().parents[1]
registry=(ROOT/'kernel/src/package/registry.rs').read_text(encoding='utf-8')
dep=(ROOT/'kernel/src/package/dependency.rs').read_text(encoding='utf-8')
assert 'dependencies_satisfied' in registry
assert 'activate_version_with_policy' in registry
assert 'VersionRequirement' in dep
assert 'PackagePolicy' in dep
assert 'policy.validate()' in registry
print('package dependency policy contract: PASS')
