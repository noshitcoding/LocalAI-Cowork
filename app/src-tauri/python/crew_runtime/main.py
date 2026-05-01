from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


def read_payload() -> dict:
    raw = sys.stdin.read().strip()
    if not raw:
        return {}
    data = json.loads(raw)
    if not isinstance(data, dict):
        raise ValueError("payload must be a JSON object")
    return data


def runtime_status() -> dict:
    crewai_installed = False
    crewai_version = None

    try:
        import crewai  # type: ignore

        crewai_installed = True
        crewai_version = getattr(crewai, "__version__", None)
    except Exception:
        crewai_installed = False

    return {
        "pythonVersion": sys.version.split()[0],
        "crewaiInstalled": crewai_installed,
        "crewaiVersion": crewai_version,
        "cwd": str(Path.cwd()),
    }


def validate_definition(payload: dict) -> dict:
    issues: list[str] = []
    normalized = {
        "name": str(payload.get("name") or "").strip(),
        "agents": payload.get("agents") or [],
        "tasks": payload.get("tasks") or [],
        "flows": payload.get("flows") or [],
    }

    if not normalized["name"]:
        issues.append("Crew-Name fehlt.")
    if not isinstance(normalized["agents"], list) or len(normalized["agents"]) == 0:
        issues.append("Mindestens ein Agent ist erforderlich.")
    if not isinstance(normalized["tasks"], list) or len(normalized["tasks"]) == 0:
        issues.append("Mindestens ein Task ist erforderlich.")

    for index, agent in enumerate(normalized["agents"]):
        if not isinstance(agent, dict):
            issues.append(f"Agent #{index + 1} hat kein gueltiges Objektformat.")
            continue
        if not str(agent.get("id") or "").strip():
            issues.append(f"Agent #{index + 1} benoetigt eine id.")
        if not str(agent.get("name") or "").strip():
            issues.append(f"Agent #{index + 1} benoetigt einen Namen.")

    for index, task in enumerate(normalized["tasks"]):
        if not isinstance(task, dict):
            issues.append(f"Task #{index + 1} hat kein gueltiges Objektformat.")
            continue
        if not str(task.get("id") or "").strip():
            issues.append(f"Task #{index + 1} benoetigt eine id.")
        if not str(task.get("agentId") or task.get("agent_id") or "").strip():
            issues.append(f"Task #{index + 1} benoetigt einen zugewiesenen Agenten.")
        if not str(task.get("description") or "").strip():
            issues.append(f"Task #{index + 1} benoetigt eine Beschreibung.")

    return {
        "valid": len(issues) == 0,
        "issues": issues,
        "normalized": normalized if len(issues) == 0 else None,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("command", choices=["status", "validate"])
    args = parser.parse_args()

    if args.command == "status":
        print(json.dumps(runtime_status()))
        return 0

    if args.command == "validate":
        payload = read_payload()
        print(json.dumps(validate_definition(payload)))
        return 0

    return 1


if __name__ == "__main__":
    raise SystemExit(main())