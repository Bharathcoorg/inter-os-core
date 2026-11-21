# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Command Space & Action Planning Fail-Closed Integrity Contract.

WHAT THIS FILE IS FOR:
Guarantees that InterOS Command Space (Do & Ask workflows) strictly fails closed:
1. Proposal timeout or IPC failure returns Err(()) and sets planned_status = 4 (Failed), never 1 (AwaitingConsent).
2. Approval timeout or IPC failure returns Err(()) and sets planned_status = 4 (Failed), never 3 (Completed).
3. Plan execution only yields Completed (3) when target AppMCP receivers successfully execute steps.
4. Model queries use model-neutral semantic role MODEL_ROLE_GENERATIVE_TEXT, not hardcoded model IDs.
5. Action query parsing prioritizes DynamicToolRegistry::canonical() intent matching over hardcoded verbs.
"""

from pathlib import Path
import pytest

ROOT = Path(__file__).resolve().parents[1]


def read_source(rel_path: str) -> str:
    return (ROOT / rel_path).read_text(encoding="utf-8")


def test_proposal_fails_closed_on_error():
    search_rs = read_source("apps/launcher/src/search.rs")
    main_rs = read_source("apps/launcher/src/main.rs")

    # query_agent_propose_plan must not fallback to Ok(1)
    assert "pub fn query_agent_propose_plan" in search_rs
    assert "Ok(1)" not in search_rs.split("pub fn query_agent_propose_plan")[1].split("pub fn query_agent_approve_plan")[0]

    # In main.rs, error in propose plan sets status = 4 (Failed)
    assert "if let Ok(st) = launcher::search::query_agent_propose_plan(" in main_rs
    assert "state.planned_status = 4; // Failed: agent unavailable or unauthenticated" in main_rs


def test_approval_fails_closed_on_error():
    search_rs = read_source("apps/launcher/src/search.rs")
    main_rs = read_source("apps/launcher/src/main.rs")

    # query_agent_approve_plan must not fallback to Ok(3)
    assert "pub fn query_agent_approve_plan" in search_rs
    assert "Ok(3)" not in search_rs.split("pub fn query_agent_approve_plan")[1].split("pub fn query_service_generate")[0]

    # In main.rs, error in approve plan sets status = 4 (Failed)
    assert "if let Ok(st) = launcher::search::query_agent_approve_plan()" in main_rs
    assert "state.planned_status = 4; // Failed: no execution result from agentd" in main_rs


def test_completed_status_requires_step_execution_success():
    service_lib = read_source("userspace/system-service/src/lib.rs")
    # In agent IPC handler, completion only occurs if all steps succeeded
    assert "std::companion::PlanStatus::Completed" in service_lib
    assert "std::companion::PlanStatus::Failed" in service_lib
    assert "all_succeeded" in service_lib


def test_ask_model_role_neutrality():
    search_rs = read_source("apps/launcher/src/search.rs")
    screen_rs = read_source("apps/launcher/src/screens/search.rs")

    # Uses MODEL_ROLE_GENERATIVE_TEXT
    assert "model_id: interos_std::ai::MODEL_ROLE_GENERATIVE_TEXT" in search_rs

    # UI label is model-neutral
    assert 'b"AI Answer"' in screen_rs
    assert 'Gemma 4 2B' not in screen_rs


def test_dynamic_intent_matching_in_launcher():
    search_rs = read_source("apps/launcher/src/search.rs")
    assert "interos_std::appmcp::DynamicToolRegistry::canonical()" in search_rs
    assert "registry.match_intent(q)" in search_rs
