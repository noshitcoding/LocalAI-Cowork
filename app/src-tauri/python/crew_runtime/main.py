from __future__ import annotations

import argparse
import contextlib
import io
import json
import os
import re
import sys
import time
import traceback
import uuid
import warnings
from pathlib import Path
from typing import Any


EXPECTED_CREWAI_VERSION = "1.15.2"
RUNTIME_SCHEMA_VERSION = 2
os.environ.setdefault("CREWAI_TRACING_ENABLED", "false")
os.environ.setdefault("OTEL_SDK_DISABLED", "true")
for runtime_stream in (sys.stdout, sys.stderr):
    try:
        runtime_stream.reconfigure(encoding="utf-8", errors="replace")
    except (AttributeError, OSError):
        pass
warnings.filterwarnings("ignore", category=DeprecationWarning, module=r"crewai(?:\.|$)")


def suppress_runtime_deprecation_warnings() -> None:
    # CrewAI currently installs warning filters while importing some modules.
    # Reapply this after its imports so deprecated internal fields do not appear
    # as red runtime stderr events in the desktop UI.
    warnings.filterwarnings("ignore", category=DeprecationWarning)


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
    tool_dependencies_installed = False

    try:
        import crewai  # type: ignore

        crewai_installed = True
        crewai_version = getattr(crewai, "__version__", None)
    except Exception:
        crewai_installed = False

    try:
        import docx  # type: ignore  # noqa: F401
        import pptx  # type: ignore  # noqa: F401

        tool_dependencies_installed = True
    except Exception:
        tool_dependencies_installed = False

    runtime_compatible = (
        crewai_installed
        and crewai_version == EXPECTED_CREWAI_VERSION
        and tool_dependencies_installed
    )
    if runtime_compatible:
        runtime_message = "Crew runtime and tool dependencies are ready."
    elif not crewai_installed:
        runtime_message = "CrewAI is not installed. Initialize the runtime."
    elif crewai_version != EXPECTED_CREWAI_VERSION:
        runtime_message = (
            f"CrewAI {crewai_version or 'unknown'} is installed, but this app requires "
            f"{EXPECTED_CREWAI_VERSION}. Reinitialize the runtime."
        )
    else:
        runtime_message = "Crew tool dependencies are incomplete. Reinitialize the runtime."

    return {
        "pythonVersion": sys.version.split()[0],
        "crewaiInstalled": crewai_installed,
        "crewaiVersion": crewai_version,
        "expectedCrewaiVersion": EXPECTED_CREWAI_VERSION,
        "toolDependenciesInstalled": tool_dependencies_installed,
        "runtimeCompatible": runtime_compatible,
        "runtimeSchemaVersion": RUNTIME_SCHEMA_VERSION,
        "runtimeMessage": runtime_message,
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
        issues.append("Crew name is missing.")
    if not isinstance(normalized["agents"], list) or len(normalized["agents"]) == 0:
        issues.append("At least one agent is required.")
    if not isinstance(normalized["tasks"], list) or len(normalized["tasks"]) == 0:
        issues.append("At least one task is required.")

    for index, agent in enumerate(normalized["agents"]):
        if not isinstance(agent, dict):
            issues.append(f"Agent #{index + 1} does not have a valid object format.")
            continue
        if not str(agent.get("id") or "").strip():
            issues.append(f"Agent #{index + 1} requires an id.")
        if not str(agent.get("name") or "").strip():
            issues.append(f"Agent #{index + 1} requires a name.")

    for index, task in enumerate(normalized["tasks"]):
        if not isinstance(task, dict):
            issues.append(f"Task #{index + 1} does not have a valid object format.")
            continue
        if not str(task.get("id") or "").strip():
            issues.append(f"Task #{index + 1} requires an id.")
        if not str(task.get("agentId") or task.get("agent_id") or "").strip():
            issues.append(f"Task #{index + 1} requires an assigned agent.")
        if not str(task.get("description") or "").strip():
            issues.append(f"Task #{index + 1} benoetigt eine Beschreibung.")

    return {
        "valid": len(issues) == 0,
        "issues": issues,
        "normalized": normalized if len(issues) == 0 else None,
    }


def now_ms() -> int:
    return int(time.time() * 1000)


ANSI_ESCAPE_RE = re.compile(r"\x1b\[[0-9;?]*[ -/]*[@-~]")


def clean_log_text(value: object) -> str:
    return ANSI_ESCAPE_RE.sub("", str(value or "").replace("\r", "\n")).strip()


def make_execution_log(
    crew_id: str,
    agent_id: str,
    task_id: str,
    action: str,
    result: object,
    *,
    agent_name: str | None = None,
    source_agent: str | None = None,
    target_agent: str | None = None,
    provider: str | None = None,
    model: str | None = None,
    task_title: str | None = None,
    phase: str | None = None,
    summary: str | None = None,
    detail: object | None = None,
    severity: str | None = None,
    provider_reasoning: str | None = None,
) -> dict:
    result_text = clean_log_text(result)[:4000]
    log = {
        "id": str(uuid.uuid4()),
        "crewId": crew_id,
        "agentId": agent_id,
        "taskId": task_id,
        "action": action,
        "result": result_text,
        "timestamp": now_ms(),
    }
    optional_fields = {
        "agentName": agent_name,
        "sourceAgent": source_agent,
        "targetAgent": target_agent,
        "provider": provider,
        "model": model,
        "taskTitle": task_title,
        "phase": phase,
        "summary": summary,
        "detail": clean_log_text(detail)[:8000] if detail is not None else None,
        "severity": severity,
        "providerReasoning": clean_log_text(provider_reasoning)[:8000] if provider_reasoning is not None else None,
    }
    for key, value in optional_fields.items():
        if isinstance(value, str) and value.strip():
            log[key] = value.strip()
    return log


def emit_protocol_log(log: dict, stream_id: str | None, run_id: str | None) -> None:
    stdout = sys.__stdout__
    if stdout is None:
        return

    envelope = {
        "openCoworkEvent": "crew_log",
        "payload": log,
    }
    if stream_id:
        envelope["streamId"] = stream_id
    if run_id:
        envelope["runId"] = run_id

    try:
        os.write(stdout.fileno(), (json.dumps(envelope) + "\n").encode("utf-8"))
    except Exception:
        pass


def record_execution_log(
    logs: list[dict],
    crew_id: str,
    agent_id: str,
    task_id: str,
    action: str,
    result: object,
    stream_id: str | None,
    run_id: str | None,
    emit: bool = True,
    *,
    agent_name: str | None = None,
    source_agent: str | None = None,
    target_agent: str | None = None,
    provider: str | None = None,
    model: str | None = None,
    task_title: str | None = None,
    phase: str | None = None,
    summary: str | None = None,
    detail: object | None = None,
    severity: str | None = None,
    provider_reasoning: str | None = None,
) -> dict | None:
    result_text = clean_log_text(result)
    if not result_text:
        return None

    log = make_execution_log(
        crew_id,
        agent_id,
        task_id,
        action,
        result_text,
        agent_name=agent_name,
        source_agent=source_agent,
        target_agent=target_agent,
        provider=provider,
        model=model,
        task_title=task_title,
        phase=phase,
        summary=summary,
        detail=detail,
        severity=severity,
        provider_reasoning=provider_reasoning,
    )
    logs.append(log)
    if emit:
        emit_protocol_log(log, stream_id, run_id)
    return log


class LiveCapture(io.StringIO):
    def __init__(
        self,
        logs: list[dict],
        crew_id: str,
        agent_id: str,
        task_id: str,
        action: str,
        stream_id: str | None,
        run_id: str | None,
    ) -> None:
        super().__init__()
        self.logs = logs
        self.crew_id = crew_id
        self.agent_id = agent_id
        self.task_id = task_id
        self.action = action
        self.stream_id = stream_id
        self.run_id = run_id
        self._pending_line = ""

    def write(self, value: str) -> int:
        written = super().write(value)
        # CrewAI frequently updates progress with carriage returns instead of newlines.
        # Treat both as live line boundaries so the UI can stream log updates promptly.
        normalized = str(value).replace("\r\n", "\n").replace("\r", "\n")
        self._pending_line += normalized

        while "\n" in self._pending_line:
            line, self._pending_line = self._pending_line.split("\n", 1)
            self._record_line(line)

        return written

    def flush(self) -> None:
        if self._pending_line.strip():
            self._record_line(self._pending_line)
            self._pending_line = ""
        super().flush()

    def _record_line(self, line: str) -> None:
        record_execution_log(
            self.logs,
            self.crew_id,
            self.agent_id,
            self.task_id,
            self.action,
            line,
            self.stream_id,
            self.run_id,
        )


def normalize_text(value: object) -> str:
    return " ".join(str(value or "").split())


def truncate_text(value: object, max_chars: int) -> str:
    normalized = normalize_text(value)
    if len(normalized) <= max_chars:
        return normalized
    return normalized[:max_chars].rstrip() + "..."


def write_json_response(payload: dict) -> None:
    """Write the protocol response while keeping late library stdout noise hidden."""
    encoded = (json.dumps(payload) + "\n").encode("utf-8")
    stdout = sys.__stdout__
    if stdout is None:
        return
    try:
        stdout.flush()
    except Exception:
        pass
    os.write(stdout.fileno(), encoded)
    sys.stdout = io.StringIO()


def format_value_list(values: object) -> str:
    if not isinstance(values, list):
        return "-"
    normalized = [str(value).strip() for value in values if str(value).strip()]
    return ", ".join(normalized) if normalized else "-"


def get_agent_governance_access(payload: dict, agent_id: str) -> dict:
    governance = payload.get("governance") or {}
    for entry in governance.get("agentAccess") or []:
        if isinstance(entry, dict) and str(entry.get("agentId") or "").strip() == agent_id:
            return entry
    return {}


def build_governance_note(payload: dict, agent_payload: dict) -> str:
    governance = payload.get("governance") or {}
    subject = str(governance.get("subject") or "").strip() or "workspace-user"
    subject_roles = format_value_list(governance.get("subjectRoles") or [])
    pending_approvals = format_value_list(governance.get("pendingApprovalTypes") or [])
    access = get_agent_governance_access(payload, str(agent_payload.get("id") or ""))

    sections: list[str] = [
        f"Execution subject: {subject}",
        f"Active roles: {subject_roles}",
    ]

    if access:
        sections.extend([
            f"Allowed tools: {format_value_list(access.get('allowedTools') or [])}",
            f"Blocked tools: {format_value_list(access.get('blockedTools') or [])}",
            f"Allowed MCP servers: {format_value_list(access.get('allowedMcpServerNames') or [])}",
            f"Blocked MCP servers: {format_value_list(access.get('blockedMcpServerNames') or [])}",
            f"Delegation allowed: {'yes' if bool(access.get('delegationAllowed')) else 'no'}",
        ])

        gateway_hints = access.get("gatewayHints") or []
        if isinstance(gateway_hints, list) and gateway_hints:
            hint_lines = [f"- {truncate_text(hint, 240)}" for hint in gateway_hints[:4]]
            sections.append("Gateway hints:\n" + "\n".join(hint_lines))

    if pending_approvals != "-":
        sections.append(f"Pending approval types: {pending_approvals}")

    return "\n".join(sections)


def build_memory_note(payload: dict) -> str:
    memory_context = payload.get("memoryContext") or {}
    sections: list[str] = []

    summary = str(memory_context.get("summary") or "").strip()
    if summary:
        sections.append(summary)

    query = str(memory_context.get("query") or "").strip()
    if query:
        sections.append(f"Knowledge query: {truncate_text(query, 240)}")

    entries = memory_context.get("entries") or []
    if isinstance(entries, list) and entries:
        entry_lines = []
        for entry in entries[:6]:
            if not isinstance(entry, dict):
                continue
            scope = str(entry.get("scope") or "shared").strip()
            category = str(entry.get("category") or "general").strip()
            key = str(entry.get("key") or "entry").strip()
            confidence = float(entry.get("confidence") or 0.0)
            content = truncate_text(entry.get("content") or "", 260)
            entry_lines.append(f"- [{scope}/{category}:{key}] ({confidence:.2f}) {content}")
        if entry_lines:
            sections.append("Relevant memory entries:\n" + "\n".join(entry_lines))

    user_profile = memory_context.get("userProfile") or []
    if isinstance(user_profile, list) and user_profile:
        profile_lines = []
        for entry in user_profile[:6]:
            if not isinstance(entry, dict):
                continue
            key = str(entry.get("key") or "profile").strip()
            value = truncate_text(entry.get("value") or "", 180)
            confidence = float(entry.get("confidence") or 0.0)
            profile_lines.append(f"- {key} ({confidence:.2f}): {value}")
        if profile_lines:
            sections.append("User profile hints:\n" + "\n".join(profile_lines))

    hints = memory_context.get("hints") or []
    if isinstance(hints, list) and hints:
        hint_lines = [f"- {truncate_text(hint, 180)}" for hint in hints[:4]]
        sections.append("Memory maintenance hints:\n" + "\n".join(hint_lines))

    return "\n\n".join(section for section in sections if section.strip())


def summarize_runtime_context(payload: dict) -> str:
    governance = payload.get("governance") or {}
    memory_context = payload.get("memoryContext") or {}
    return " | ".join([
        f"subject={str(governance.get('subject') or 'workspace-user').strip() or 'workspace-user'}",
        f"roles={format_value_list(governance.get('subjectRoles') or [])}",
        f"pendingApprovals={format_value_list(governance.get('pendingApprovalTypes') or [])}",
        f"memoryEntries={len(memory_context.get('entries') or []) if isinstance(memory_context.get('entries') or [], list) else 0}",
        f"profileHints={len(memory_context.get('userProfile') or []) if isinstance(memory_context.get('userProfile') or [], list) else 0}",
    ])


def agent_display_name(agent_payload: dict, fallback: str) -> str:
    return str(agent_payload.get("name") or agent_payload.get("role") or fallback).strip() or fallback


def task_display_title(task_payload: dict, fallback: str) -> str:
    title = str(task_payload.get("title") or task_payload.get("name") or "").strip()
    if title:
        return title
    description = truncate_text(task_payload.get("description") or "", 80)
    return description or fallback


def resolve_agent_provider(agent_payload: dict) -> str:
    return str(agent_payload.get("providerKind") or "ollama").strip() or "ollama"


def resolve_agent_model_label(request: dict, agent_payload: dict) -> str:
    provider = resolve_agent_provider(agent_payload)
    model_override = str(agent_payload.get("modelOverride") or "").strip()
    if model_override:
        return model_override

    provider_configs = request.get("providerConfigs") or {}
    if provider == "openai-compatible":
        config = provider_configs.get("openAICompatible") or {}
        return str(config.get("model") or "").strip() or "-"
    if provider == "openrouter":
        config = provider_configs.get("openRouter") or {}
        return str(config.get("model") or "").strip() or "-"

    request_config = request.get("config") or {}
    return str(request_config.get("model") or "").strip() or "-"


def find_agent_payload(agent_payloads: list, agent_id: str) -> dict:
    return next(
        (
            candidate
            for candidate in agent_payloads
            if isinstance(candidate, dict) and str(candidate.get("id") or "").strip() == agent_id
        ),
        {},
    )


RETIRED_OPENROUTER_MODELS = {
    "tencent/hy3-preview:free": "Hy3 preview is no longer available as a free model on OpenRouter.",
}


def parse_int(value: object, fallback: int = 0) -> int:
    try:
        return int(value or fallback)
    except Exception:
        return fallback


def is_openrouter_free_model(model: str) -> bool:
    return str(model or "").strip().lower().endswith(":free")


def openrouter_model_id(model: str) -> str:
    normalized = str(model or "").strip().lower()
    if normalized.startswith("openrouter/"):
        return normalized[len("openrouter/"):]
    return normalized


def validate_runtime_provider_models(payload: dict, agent_payloads: list[dict]) -> None:
    invalid_models: list[str] = []
    free_models: list[str] = []
    has_openrouter_agent = False

    for agent_payload in agent_payloads:
        if not isinstance(agent_payload, dict):
            continue
        provider = resolve_agent_provider(agent_payload)
        if provider != "openrouter":
            continue
        has_openrouter_agent = True
        model = resolve_agent_model_label(payload, agent_payload)
        model_id = openrouter_model_id(model)
        if model_id in RETIRED_OPENROUTER_MODELS:
            invalid_models.append(f"{model} ({RETIRED_OPENROUTER_MODELS[model_id]})")
        elif is_openrouter_free_model(model):
            free_models.append(model)

    if has_openrouter_agent:
        provider_configs = payload.get("providerConfigs") or {}
        openrouter_config = provider_configs.get("openRouter") or {}
        if not str(openrouter_config.get("apiKey") or "").strip():
            raise ValueError(
                "OpenRouter API key is missing. Add it to the default OpenRouter LLM profile "
                "or this Crew's OpenRouter provider profile before starting the Crew."
            )

    if invalid_models:
        raise ValueError(
            "OpenRouter model unavailable: "
            + "; ".join(invalid_models)
            + " Select a current model or a paid/API-key-backed provider."
        )

    if free_models and parse_int(payload.get("maxParallelTasks"), 1) > 1:
        payload["maxParallelTasks"] = min(2, parse_int(payload.get("maxParallelTasks"), 1))
        payload["_rateLimitPolicyReason"] = (
            "OpenRouter free models use bounded parallel execution (max 2) with up to 4 provider retries."
        )


def has_openrouter_free_agent(payload: dict, agent_payloads: list[dict]) -> bool:
    for agent_payload in agent_payloads:
        if not isinstance(agent_payload, dict):
            continue
        if resolve_agent_provider(agent_payload) == "openrouter" and is_openrouter_free_model(resolve_agent_model_label(payload, agent_payload)):
            return True
    return False


def effective_max_rpm(payload: dict, agent_count: int, uses_openrouter_free: bool) -> int:
    requested = parse_int(payload.get("maxRpm"), 0)
    if requested <= 0:
        return 0

    if uses_openrouter_free:
        requested = min(requested, 2)

    return max(1, requested // max(1, agent_count))


def classify_runtime_error(exc: Exception) -> str:
    raw = f"{exc.__class__.__name__}: {exc}"
    lowered = raw.lower()

    if (
        "rate_limit" in lowered
        or "ratelimit" in lowered
        or "resourceexhausted" in lowered
        or "request limit reached" in lowered
        or "429" in lowered
    ):
        return (
            "RateLimitError: The provider rejected too many or too rapid requests. "
            "The crew was stopped. Reduce max RPM/parallelism or use a provider profile with its own API key. "
            f"Original: {raw}"
        )

    if "model" in lowered and ("not found" in lowered or "404" in lowered):
        return (
            "ModelNotFoundError: The configured model is not available from the provider. "
            "Select a current model in Crew/LLM profiles. "
            f"Original: {raw}"
        )

    if "authentication" in lowered or "unauthorized" in lowered or "401" in lowered:
        return (
            "AuthenticationError: The configured provider rejected the credentials. "
            "Check the selected Crew provider profile and API key. "
            f"Original: {raw}"
        )

    if "authentication" in lowered or "unauthorized" in lowered or "401" in lowered:
        return (
            "AuthenticationError: The configured provider rejected the credentials. "
            "Check the selected Crew provider profile and API key. "
            f"Original: {raw}"
        )

    if "connection" in lowered or "connecterror" in lowered or "connection refused" in lowered:
        return (
            "ConnectionError: The configured model provider is unreachable. "
            "Check its base URL and whether the local/provider service is running. "
            f"Original: {raw}"
        )

    if "timeout" in lowered or "timed out" in lowered:
        return (
            "TimeoutError: The provider or a Crew tool exceeded its time limit. "
            "Retry the task or increase the configured runtime timeout. "
            f"Original: {raw}"
        )

    if "utf-8" in lowered or "unicode" in lowered:
        return (
            "ConnectionError: The configured model provider is unreachable. "
            "Check its base URL and whether the local/provider service is running. "
            f"Original: {raw}"
        )

    if "timeout" in lowered or "timed out" in lowered:
        return (
            "TimeoutError: The provider or a Crew tool exceeded its time limit. "
            "Retry the task or increase the configured runtime timeout. "
            f"Original: {raw}"
        )

    if "utf-8" in lowered or "unicode" in lowered:
        return (
            "EncodingError: The runtime received non-UTF-8 output. "
            "Output is now read losslessly; please start again. "
            f"Original: {raw}"
        )

    return raw


def mark_recovered_provider_logs(logs: list[dict], status: str) -> None:
    if status != "completed":
        return
    retryable_markers = (
        "resourceexhausted",
        "request limit reached",
        "rate_limit",
        "ratelimit",
        "status code: 429",
    )
    for entry in logs:
        if entry.get("action") != "runtime_stderr":
            continue
        result = str(entry.get("result") or "")
        if not any(marker in result.lower() for marker in retryable_markers):
            continue
        entry.update({
            "action": "provider_retry_recovered",
            "agentName": "Runtime",
            "phase": "status",
            "summary": "Provider retry recovered",
            "detail": result,
            "severity": "warning",
        })


def normalize_model_name(provider: str, model: str) -> str:
    normalized = str(model or "").strip()
    if not normalized:
        raise ValueError(f"No model configured for provider '{provider}'.")
    if provider == "openrouter":
        return normalized if normalized.startswith("openrouter/") else f"openrouter/{normalized}"
    if provider == "openai-compatible":
        return normalized if normalized.startswith("openai/") else f"openai/{normalized}"
    return normalized if normalized.startswith("ollama/") else f"ollama/{normalized}"


def configure_litellm_tls_verification(verify_tls_certificates: object) -> None:
    if verify_tls_certificates is not False:
        return
    retryable_markers = (
        "resourceexhausted",
        "request limit reached",
        "rate_limit",
        "ratelimit",
        "status code: 429",
    )
    for entry in logs:
        if entry.get("action") != "runtime_stderr":
            continue
        result = str(entry.get("result") or "")
        if not any(marker in result.lower() for marker in retryable_markers):
            continue
        entry.update({
            "action": "provider_retry_recovered",
            "agentName": "Runtime",
            "phase": "status",
            "summary": "Provider retry recovered",
            "detail": result,
            "severity": "warning",
        })


def normalize_model_name(provider: str, model: str) -> str:
    normalized = str(model or "").strip()
    if not normalized:
        raise ValueError(f"No model configured for provider '{provider}'.")
    if provider == "openrouter":
        return normalized if normalized.startswith("openrouter/") else f"openrouter/{normalized}"
    if provider == "openai-compatible":
        return normalized if normalized.startswith("openai/") else f"openai/{normalized}"
    return normalized if normalized.startswith("ollama/") else f"ollama/{normalized}"


def configure_litellm_tls_verification(verify_tls_certificates: object) -> None:
    if verify_tls_certificates is not False:
        return

    try:
        import httpx  # type: ignore
        import litellm  # type: ignore

        litellm.client_session = httpx.Client(verify=False)
        litellm.aclient_session = httpx.AsyncClient(verify=False)
    except Exception:
        pass


def resolve_agent_timeout_ms(request: dict, agent: dict) -> int:
    request_config = request.get("config") or {}
    provider_configs = request.get("providerConfigs") or {}
    provider = resolve_agent_provider(agent)
    provider_config: dict = {}
    if provider == "openrouter":
        provider_config = provider_configs.get("openRouter") or {}
    elif provider == "openai-compatible":
        provider_config = provider_configs.get("openAICompatible") or {}

    fallback = parse_int(request_config.get("timeoutMs"), 600_000)
    return max(1_000, parse_int(provider_config.get("timeoutMs"), fallback))


def resolve_llm_max_retries(request: dict, model: str) -> int:
    configured = max(0, min(5, parse_int(request.get("retryCount"), 0)))
    provider_default = 4 if is_openrouter_free_model(model) else 2
    return max(provider_default, configured)


def _openai_tool_definitions(tools: list[dict] | None) -> dict[str, dict]:
    definitions: dict[str, dict] = {}
    for tool in tools or []:
        if not isinstance(tool, dict):
            continue
        function = tool.get("function") if isinstance(tool.get("function"), dict) else tool
        name = str(function.get("name") or "").strip()
        if name:
            definitions[name] = function
    return definitions


def _decode_textual_tool_payloads(text: str) -> list[Any] | None:
    candidate = text.strip()
    fenced = re.fullmatch(r"```(?:json)?\s*(.*?)\s*```", candidate, flags=re.DOTALL | re.IGNORECASE)
    if fenced:
        candidate = fenced.group(1).strip()
    tagged = re.fullmatch(r"<tool_call>\s*(.*?)\s*</tool_call>", candidate, flags=re.DOTALL | re.IGNORECASE)
    if tagged:
        candidate = tagged.group(1).strip()
    decoder = json.JSONDecoder()
    payloads: list[Any] = []
    offset = 0
    try:
        while offset < len(candidate):
            while offset < len(candidate) and candidate[offset].isspace():
                offset += 1
            if offset >= len(candidate):
                break
            payload, consumed = decoder.raw_decode(candidate, offset)
            payloads.append(payload)
            offset = consumed
    except (TypeError, ValueError):
        return None
    return payloads or None


def _decode_tool_arguments(value: Any) -> dict | None:
    if isinstance(value, dict):
        return value
    if isinstance(value, str):
        try:
            decoded = json.loads(value)
        except ValueError:
            return None
        return decoded if isinstance(decoded, dict) else None
    return None


def _named_textual_tool_call(payload: dict, definitions: dict[str, dict]) -> tuple[str, dict] | None:
    if isinstance(payload.get("function"), dict):
        function = payload["function"]
        name = str(function.get("name") or "").strip()
        arguments = _decode_tool_arguments(function.get("arguments"))
    else:
        name = str(payload.get("tool") or payload.get("tool_name") or payload.get("name") or "").strip()
        raw_arguments = next(
            (
                payload[key]
                for key in ("arguments", "args", "input", "parameters")
                if key in payload
            ),
            None,
        )
        arguments = _decode_tool_arguments(raw_arguments)
    if name in definitions and arguments is not None:
        return name, arguments
    return None


def _inferred_textual_tool_call(payload: dict, definitions: dict[str, dict]) -> tuple[str, dict] | None:
    payload_keys = set(payload)
    if not payload_keys:
        return None
    matches: list[tuple[int, str]] = []
    for name, definition in definitions.items():
        parameters = definition.get("parameters") if isinstance(definition.get("parameters"), dict) else {}
        properties = parameters.get("properties") if isinstance(parameters.get("properties"), dict) else {}
        required = {
            str(value)
            for value in parameters.get("required") or []
            if str(value).strip()
        }
        property_names = set(properties)
        if not property_names or not payload_keys.issubset(property_names) or not required.issubset(payload_keys):
            continue
        score = len(payload_keys) * 10 + len(required)
        matches.append((score, name))
    matches.sort(reverse=True)
    if not matches or (len(matches) > 1 and matches[0][0] == matches[1][0]):
        return None
    return matches[0][1], payload


def bridge_textual_tool_call(result: Any, tools: list[dict] | None) -> Any:
    """Turn strict JSON tool-call text into the native structure CrewAI executes.

    Some OpenAI-compatible models return a requested function call in the text
    content instead of ``message.tool_calls``. Only a complete JSON value (or a
    JSON value wrapped by a tool-call tag/fence) is accepted, and it must match
    one of the schemas CrewAI supplied for the current agent.
    """
    if not isinstance(result, str) or not result.strip() or not tools:
        return result
    definitions = _openai_tool_definitions(tools)
    payloads = _decode_textual_tool_payloads(result)
    if not definitions or payloads is None:
        return result
    payload = payloads[0]
    if isinstance(payload, list):
        payload = payload[0] if payload else None
    if isinstance(payload, dict) and isinstance(payload.get("tool_calls"), list):
        payload = payload["tool_calls"][0] if payload["tool_calls"] else None
    if not isinstance(payload, dict):
        return result
    parsed = _named_textual_tool_call(payload, definitions) or _inferred_textual_tool_call(payload, definitions)
    if parsed is None:
        return result
    name, arguments = parsed
    return [{
        "id": f"open_cowork_text_call_{uuid.uuid4().hex}",
        "type": "function",
        "function": {"name": name, "arguments": json.dumps(arguments)},
    }]


_RUNTIME_LLM_CLASS = None


def runtime_llm_class():
    global _RUNTIME_LLM_CLASS
    if _RUNTIME_LLM_CLASS is not None:
        return _RUNTIME_LLM_CLASS

    from crewai import LLM  # type: ignore

    class RuntimeToolCompatibleLLM(LLM):
        def supports_function_calling(self) -> bool:
            if getattr(self, "_open_cowork_force_native_tools", False):
                return True
            return super().supports_function_calling()

        def call(self, *args, **kwargs):
            tools = kwargs.get("tools") if "tools" in kwargs else (args[1] if len(args) > 1 else None)
            result = super().call(*args, **kwargs)
            return bridge_textual_tool_call(result, tools)

        async def acall(self, *args, **kwargs):
            tools = kwargs.get("tools") if "tools" in kwargs else (args[1] if len(args) > 1 else None)
            result = await super().acall(*args, **kwargs)
            return bridge_textual_tool_call(result, tools)

    _RUNTIME_LLM_CLASS = RuntimeToolCompatibleLLM
    return _RUNTIME_LLM_CLASS


def build_llm(request: dict, agent: dict):
    suppress_runtime_deprecation_warnings()
    LLM = runtime_llm_class()

    provider = resolve_agent_provider(agent)
    model_override = str(agent.get("modelOverride") or "").strip()
    request_config = request.get("config") or {}
    provider_configs = request.get("providerConfigs") or {}
    provider = resolve_agent_provider(agent)
    provider_config: dict = {}
    if provider == "openrouter":
        provider_config = provider_configs.get("openRouter") or {}
    elif provider == "openai-compatible":
        provider_config = provider_configs.get("openAICompatible") or {}

    fallback = parse_int(request_config.get("timeoutMs"), 600_000)
    return max(1_000, parse_int(provider_config.get("timeoutMs"), fallback))


def resolve_llm_max_retries(request: dict, model: str) -> int:
    configured = max(0, min(5, parse_int(request.get("retryCount"), 0)))
    provider_default = 4 if is_openrouter_free_model(model) else 2
    return max(provider_default, configured)


def _openai_tool_definitions(tools: list[dict] | None) -> dict[str, dict]:
    definitions: dict[str, dict] = {}
    for tool in tools or []:
        if not isinstance(tool, dict):
            continue
        function = tool.get("function") if isinstance(tool.get("function"), dict) else tool
        name = str(function.get("name") or "").strip()
        if name:
            definitions[name] = function
    return definitions


def _decode_textual_tool_payloads(text: str) -> list[Any] | None:
    candidate = text.strip()
    fenced = re.fullmatch(r"```(?:json)?\s*(.*?)\s*```", candidate, flags=re.DOTALL | re.IGNORECASE)
    if fenced:
        candidate = fenced.group(1).strip()
    tagged = re.fullmatch(r"<tool_call>\s*(.*?)\s*</tool_call>", candidate, flags=re.DOTALL | re.IGNORECASE)
    if tagged:
        candidate = tagged.group(1).strip()
    decoder = json.JSONDecoder()
    payloads: list[Any] = []
    offset = 0
    try:
        while offset < len(candidate):
            while offset < len(candidate) and candidate[offset].isspace():
                offset += 1
            if offset >= len(candidate):
                break
            payload, consumed = decoder.raw_decode(candidate, offset)
            payloads.append(payload)
            offset = consumed
    except (TypeError, ValueError):
        return None
    return payloads or None


def _decode_tool_arguments(value: Any) -> dict | None:
    if isinstance(value, dict):
        return value
    if isinstance(value, str):
        try:
            decoded = json.loads(value)
        except ValueError:
            return None
        return decoded if isinstance(decoded, dict) else None
    return None


def _named_textual_tool_call(payload: dict, definitions: dict[str, dict]) -> tuple[str, dict] | None:
    if isinstance(payload.get("function"), dict):
        function = payload["function"]
        name = str(function.get("name") or "").strip()
        arguments = _decode_tool_arguments(function.get("arguments"))
    else:
        name = str(payload.get("tool") or payload.get("tool_name") or payload.get("name") or "").strip()
        raw_arguments = next(
            (
                payload[key]
                for key in ("arguments", "args", "input", "parameters")
                if key in payload
            ),
            None,
        )
        arguments = _decode_tool_arguments(raw_arguments)
    if name in definitions and arguments is not None:
        return name, arguments
    return None


def _inferred_textual_tool_call(payload: dict, definitions: dict[str, dict]) -> tuple[str, dict] | None:
    payload_keys = set(payload)
    if not payload_keys:
        return None
    matches: list[tuple[int, str]] = []
    for name, definition in definitions.items():
        parameters = definition.get("parameters") if isinstance(definition.get("parameters"), dict) else {}
        properties = parameters.get("properties") if isinstance(parameters.get("properties"), dict) else {}
        required = {
            str(value)
            for value in parameters.get("required") or []
            if str(value).strip()
        }
        property_names = set(properties)
        if not property_names or not payload_keys.issubset(property_names) or not required.issubset(payload_keys):
            continue
        score = len(payload_keys) * 10 + len(required)
        matches.append((score, name))
    matches.sort(reverse=True)
    if not matches or (len(matches) > 1 and matches[0][0] == matches[1][0]):
        return None
    return matches[0][1], payload


def bridge_textual_tool_call(result: Any, tools: list[dict] | None) -> Any:
    """Turn strict JSON tool-call text into the native structure CrewAI executes.

    Some OpenAI-compatible models return a requested function call in the text
    content instead of ``message.tool_calls``. Only a complete JSON value (or a
    JSON value wrapped by a tool-call tag/fence) is accepted, and it must match
    one of the schemas CrewAI supplied for the current agent.
    """
    if not isinstance(result, str) or not result.strip() or not tools:
        return result
    definitions = _openai_tool_definitions(tools)
    payloads = _decode_textual_tool_payloads(result)
    if not definitions or payloads is None:
        return result
    payload = payloads[0]
    if isinstance(payload, list):
        payload = payload[0] if payload else None
    if isinstance(payload, dict) and isinstance(payload.get("tool_calls"), list):
        payload = payload["tool_calls"][0] if payload["tool_calls"] else None
    if not isinstance(payload, dict):
        return result
    parsed = _named_textual_tool_call(payload, definitions) or _inferred_textual_tool_call(payload, definitions)
    if parsed is None:
        return result
    name, arguments = parsed
    return [{
        "id": f"open_cowork_text_call_{uuid.uuid4().hex}",
        "type": "function",
        "function": {"name": name, "arguments": json.dumps(arguments)},
    }]


_RUNTIME_LLM_CLASS = None


def runtime_llm_class():
    global _RUNTIME_LLM_CLASS
    if _RUNTIME_LLM_CLASS is not None:
        return _RUNTIME_LLM_CLASS

    from crewai import LLM  # type: ignore

    class RuntimeToolCompatibleLLM(LLM):
        def supports_function_calling(self) -> bool:
            if getattr(self, "_open_cowork_force_native_tools", False):
                return True
            return super().supports_function_calling()

        def call(self, *args, **kwargs):
            tools = kwargs.get("tools") if "tools" in kwargs else (args[1] if len(args) > 1 else None)
            result = super().call(*args, **kwargs)
            return bridge_textual_tool_call(result, tools)

        async def acall(self, *args, **kwargs):
            tools = kwargs.get("tools") if "tools" in kwargs else (args[1] if len(args) > 1 else None)
            result = await super().acall(*args, **kwargs)
            return bridge_textual_tool_call(result, tools)

    _RUNTIME_LLM_CLASS = RuntimeToolCompatibleLLM
    return _RUNTIME_LLM_CLASS


def build_llm(request: dict, agent: dict):
    suppress_runtime_deprecation_warnings()
    LLM = runtime_llm_class()

    provider = resolve_agent_provider(agent)
    model_override = str(agent.get("modelOverride") or "").strip()
    request_config = request.get("config") or {}
    provider_configs = request.get("providerConfigs") or {}

    if provider == "openai-compatible":
        config = provider_configs.get("openAICompatible") or {}
        configure_litellm_tls_verification(config.get("verifyTlsCertificates"))
        model = normalize_model_name(provider, model_override or str(config.get("model") or ""))
        timeout_seconds = resolve_agent_timeout_ms(request, agent) // 1000
        llm_kwargs = {
            "model": model,
            "base_url": str(config.get("baseUrl") or request_config.get("baseUrl") or "https://api.openai.com/v1"),
            "api_key": str(config.get("apiKey") or "open-cowork"),
            "timeout": timeout_seconds,
            "max_retries": resolve_llm_max_retries(request, model),
            "max_tokens": 4096,
            "_skip_mcp_handler": True,
        }
        return LLM(**llm_kwargs)

    if provider == "openrouter":
        config = provider_configs.get("openRouter") or {}
        configure_litellm_tls_verification(config.get("verifyTlsCertificates"))
        model = normalize_model_name(provider, model_override or str(config.get("model") or ""))
        api_key = str(config.get("apiKey") or "").strip()
        if not api_key:
            raise ValueError(
                "OpenRouter API key is missing. Configure the default OpenRouter LLM profile before starting the Crew."
            )
        timeout_seconds = resolve_agent_timeout_ms(request, agent) // 1000
        llm_kwargs = {
            "model": model,
            "is_litellm": True,
            "base_url": str(config.get("baseUrl") or "https://openrouter.ai/api/v1"),
            "api_key": api_key,
            "timeout": timeout_seconds,
            "max_retries": resolve_llm_max_retries(request, model),
            "max_tokens": 4096,
            "_skip_mcp_handler": True,
        }
        llm = LLM(**llm_kwargs)
        # OpenRouter models may support the OpenAI tools request while LiteLLM's
        # static capability registry still reports False (notably Nemotron).
        # The runtime bridge safely handles models that put the call in content.
        setattr(llm, "_open_cowork_force_native_tools", True)
        return llm

    model = normalize_model_name(provider, model_override or str(request_config.get("model") or ""))
    timeout_seconds = resolve_agent_timeout_ms(request, agent) // 1000
    return LLM(
        model=model,
        base_url=str(request_config.get("baseUrl") or "http://localhost:11434"),
        timeout=timeout_seconds,
        max_tokens=4096,
    )


def build_agent(request: dict, agent_payload: dict):
    from crewai import Agent  # type: ignore
    from crew_tools import build_runtime_tools, unavailable_runtime_tools
    suppress_runtime_deprecation_warnings()

    skills_markdown = str(agent_payload.get("skillsMarkdown") or "").strip()
    backstory = str(agent_payload.get("backstory") or "").strip()
    if skills_markdown:
        backstory = f"{backstory}\n\nSkills:\n{skills_markdown}".strip()

    governance_note = build_governance_note(request, agent_payload)
    if governance_note:
        backstory = f"{backstory}\n\nGovernance:\n{governance_note}".strip()

    runtime_tools = build_runtime_tools(request, agent_payload)
    unavailable_tools = unavailable_runtime_tools(request, agent_payload)
    if unavailable_tools:
        backstory = (
            f"{backstory}\n\nUnavailable runtime integrations: {', '.join(unavailable_tools)}. "
            "Do not claim to have used them."
        ).strip()

    max_rpm = int(agent_payload.get("maxRpm") or request.get("_effectiveAgentMaxRpm") or 0)
    retry_count = max(0, min(5, parse_int(request.get("retryCount"), 0)))
    timeout_ms = resolve_agent_timeout_ms(request, agent_payload)

    agent_kwargs = {
        "role": str(agent_payload.get("role") or agent_payload.get("name") or "Crew Agent"),
        "goal": str(agent_payload.get("goal") or "Complete tasks successfully in the crew."),
        "backstory": backstory or "A specialized crew agent for Open_Cowork.",
        "llm": build_llm(request, agent_payload),
        "tools": runtime_tools,
        "verbose": bool(agent_payload.get("verbose")),
        "allow_delegation": bool(agent_payload.get("allowDelegation")),
        "max_iter": max(1, int(agent_payload.get("maxIterations") or 20)),
        "max_retry_limit": retry_count,
        "max_execution_time": max(1, timeout_ms // 1000),
        "respect_context_window": True,
        "cache": True,
    }
    if max_rpm > 0:
        agent_kwargs["max_rpm"] = max_rpm

    return Agent(**agent_kwargs)

    max_rpm = int(agent_payload.get("maxRpm") or request.get("_effectiveAgentMaxRpm") or 0)
    retry_count = max(0, min(5, parse_int(request.get("retryCount"), 0)))
    timeout_ms = resolve_agent_timeout_ms(request, agent_payload)

    agent_kwargs = {
        "role": str(agent_payload.get("role") or agent_payload.get("name") or "Crew Agent"),
        "goal": str(agent_payload.get("goal") or "Complete tasks successfully in the crew."),
        "backstory": backstory or "A specialized crew agent for Open_Cowork.",
        "llm": build_llm(request, agent_payload),
        "tools": runtime_tools,
        "verbose": bool(agent_payload.get("verbose")),
        "allow_delegation": bool(agent_payload.get("allowDelegation")),
        "max_iter": max(1, int(agent_payload.get("maxIterations") or 20)),
        "max_retry_limit": retry_count,
        "max_execution_time": max(1, timeout_ms // 1000),
        "respect_context_window": True,
        "cache": True,
    }
    if max_rpm > 0:
        agent_kwargs["max_rpm"] = max_rpm

    return Agent(**agent_kwargs)


def build_task_description(request: dict, task_payload: dict, agent_payload: dict) -> str:
    description = str(task_payload.get("description") or "").strip()
    execution_guidelines = str(request.get("executionGuidelines") or "").strip()
    knowledge_focus = str(request.get("knowledgeFocus") or "").strip()
    cwd = str(request.get("cwd") or "").strip()
    governance_note = build_governance_note(request, agent_payload)
    memory_note = build_memory_note(request)

    additions = []
    if execution_guidelines:
        additions.append(f"Crew guidelines:\n{execution_guidelines}")
    if knowledge_focus:
        additions.append(f"Knowledge focus:\n{knowledge_focus}")
    if cwd:
        additions.append(f"Working directory: {cwd}")
    if governance_note:
        additions.append(f"Governance context:\n{governance_note}")
    if memory_note:
        additions.append(f"Crew memory and knowledge:\n{memory_note}")

    output_mode = str(request.get("outputMode") or "standard").strip().lower()
    if output_mode == "bullet-report":
        additions.append("Output contract: return a concise bullet report with explicit evidence and artifact paths.")
    elif output_mode == "json":
        additions.append("Output contract: return valid JSON only, without Markdown fences or commentary outside the JSON value.")

    additions.append(
        "Execution contract:\n"
        "- Use the provided runtime tools for facts, files, commands, and artifacts; never pretend a tool was called.\n"
        "- For research, include the source URLs returned by web_search/web_fetch.\n"
        "- For coding, inspect existing files first, make the smallest complete change, and run relevant verification.\n"
        "- For PPTX/DOCX work, call office_workflow directly with output_path, title, and a sections_json JSON array; "
        "do not describe or print a proposed tool call. Report the exact path returned by the tool.\n"
        "- If a required tool returns ERROR, explain the concrete blocker instead of fabricating a result."
    )

    if additions:
        return f"{description}\n\n" + "\n\n".join(additions)
    return description


    additions.append(
        "Execution contract:\n"
        "- Use the provided runtime tools for facts, files, commands, and artifacts; never pretend a tool was called.\n"
        "- For research, include the source URLs returned by web_search/web_fetch.\n"
        "- For coding, inspect existing files first, make the smallest complete change, and run relevant verification.\n"
        "- For PPTX/DOCX work, call office_workflow directly with output_path, title, and a sections_json JSON array; "
        "do not describe or print a proposed tool call. Report the exact path returned by the tool.\n"
        "- If a required tool returns ERROR, explain the concrete blocker instead of fabricating a result."
    )

    if additions:
        return f"{description}\n\n" + "\n\n".join(additions)
    return description


def dedupe_task_refs(values: list[str]) -> list[str]:
    seen: set[str] = set()
    ordered: list[str] = []
    for value in values:
        if value in seen:
            continue
        seen.add(value)
        ordered.append(value)
    return ordered


def order_task_payloads(task_payloads: list[dict]) -> list[dict]:
    """Stable topological order so CrewAI never executes a dependent task first."""
    by_id = {
        str(task.get("id") or "").strip(): task
        for task in task_payloads
        if isinstance(task, dict) and str(task.get("id") or "").strip()
    }
    ordered: list[dict] = []
    visiting: set[str] = set()
    visited: set[str] = set()

    def visit(task_id: str) -> None:
        if task_id in visited:
            return
        if task_id in visiting:
            raise ValueError(f"Crew task dependency cycle detected at {task_id}.")
        visiting.add(task_id)
        task = by_id[task_id]
        refs = dedupe_task_refs([
            str(value).strip()
            for value in [*(task.get("context") or []), *(task.get("dependencies") or [])]
            if str(value).strip()
        ])
        for ref in refs:
            if ref not in by_id:
                raise ValueError(f"Task {task_id} references unknown context/dependency {ref}.")
            visit(ref)
        visiting.remove(task_id)
        visited.add(task_id)
        ordered.append(task)

    for task in task_payloads:
        if isinstance(task, dict):
            task_id = str(task.get("id") or "").strip()
            if task_id:
                visit(task_id)
    return ordered


def normalize_task_concurrency(
    task_payloads: list[dict],
    process_name: str,
    max_parallel_tasks: int,
) -> tuple[str, list[dict]]:
    limit = max(1, max_parallel_tasks)
    if limit <= 1:
        return "sequential", [
            {**task_payload, "asyncExecution": False}
            for task_payload in task_payloads
        ]
    if process_name.lower() == "parallel":
        remaining = list(task_payloads)
        completed: set[str] = set()
        normalized: list[dict] = []
        batch_index = 0
        while remaining:
            ready = []
            for task_payload in remaining:
                refs = dedupe_task_refs([
                    str(value).strip()
                    for value in [
                        *(task_payload.get("context") or []),
                        *(task_payload.get("dependencies") or []),
                    ]
                    if str(value).strip()
                ])
                if all(ref in completed for ref in refs):
                    ready.append(task_payload)
            batch = ready[:limit]
            if not batch:
                raise ValueError("Crew task dependency graph cannot produce a parallel execution batch.")
            is_parallel_batch = len(batch) > 1
            for task_payload in batch:
                normalized.append({
                    **task_payload,
                    "asyncExecution": is_parallel_batch,
                    "_parallelBatch": batch_index,
                })
                task_id = str(task_payload.get("id") or "").strip()
                if task_id:
                    completed.add(task_id)
                remaining.remove(task_payload)
            batch_index += 1
        return process_name, normalized
    normalized = [dict(task_payload) for task_payload in task_payloads]
    if normalized:
        # Defensive compatibility for imported definitions: the final task must
        # be synchronous or CrewAI validation fails before kickoff.
        normalized[-1]["asyncExecution"] = False
    return process_name, normalized


def execute_parallel_crew(crew, ordered_task_bindings: list[tuple[dict, object]]) -> None:
    """Execute dependency-safe task batches concurrently.

    CrewAI 1.15.2 waits for pending async tasks before it starts the next
    synchronous task, so using a sync task as a batch boundary accidentally
    serializes a two-task batch. The runtime prepares the Crew normally, then
    starts every task in one dependency batch before waiting for its futures.
    """
    from crewai.crews.utils import prepare_kickoff  # type: ignore

    prepare_kickoff(crew, None)
    task_outputs = []
    task_indexes = {
        id(task_obj): task_index
        for task_index, (_, task_obj) in enumerate(ordered_task_bindings)
    }
    batch_ids = sorted({
        int(task_payload.get("_parallelBatch") or 0)
        for task_payload, _ in ordered_task_bindings
    })
    for batch_id in batch_ids:
        batch = [
            (task_payload, task_obj)
            for task_payload, task_obj in ordered_task_bindings
            if int(task_payload.get("_parallelBatch") or 0) == batch_id
        ]
        futures = []
        for _, task_obj in batch:
            task_index = task_indexes[id(task_obj)]
            agent = getattr(task_obj, "agent", None)
            if agent is None:
                raise ValueError("Parallel Crew task has no assigned agent.")
            tools = getattr(task_obj, "tools", None) or getattr(agent, "tools", None) or []
            tools = crew._prepare_tools(agent, task_obj, tools)
            crew._log_task_start(task_obj, agent.role)
            context = crew._get_context(task_obj, task_outputs)
            future = task_obj.execute_async(agent=agent, context=context, tools=tools)
            futures.append((task_index, task_obj, future))

        batch_outputs = []
        for task_index, task_obj, future in futures:
            task_output = future.result()
            batch_outputs.append(task_output)
            crew._process_task_result(task_obj, task_output)
            crew._store_execution_log(task_obj, task_output, task_index)
        task_outputs.extend(batch_outputs)


def extract_task_output(task_obj) -> str | None:
    output = getattr(task_obj, "output", None)
    if output is None:
        return None
    raw = getattr(output, "raw", None)
    if isinstance(raw, str) and raw.strip():
        return raw.strip()
    try:
        rendered = str(output).strip()
    except Exception:
        return None
    return rendered or None


OFFICE_ARTIFACT_PATTERN = re.compile(
    r"(?i)(?P<path>(?:[A-Za-z]:[\\/])?[A-Za-z0-9_.-]+(?:[\\/][A-Za-z0-9_.-]+)*\.(?:pptx|docx))"
)
EDIT_ARTIFACT_PATTERN = re.compile(
    r"(?i)(?P<path>(?:[A-Za-z]:[\\/])?[A-Za-z0-9_.-]+(?:[\\/][A-Za-z0-9_.-]+)*\.(?:md|py|json|csv|txt|ts|tsx|js|jsx|rs))"
)


def expected_office_artifact_paths(request: dict, task_payload: dict, agent_payload: dict) -> list[Path]:
    tools = {str(value or "").strip().lower().replace("-", "_") for value in agent_payload.get("tools") or []}
    if "office_workflow" not in tools:
        return []

    root = Path(str(request.get("cwd") or Path.cwd())).expanduser().resolve(strict=False)
    task_contract = "\n".join([
        str(task_payload.get("description") or ""),
        str(task_payload.get("expectedOutput") or ""),
    ])
    paths: list[Path] = []
    seen: set[Path] = set()
    for match in OFFICE_ARTIFACT_PATTERN.finditer(task_contract):
        value = match.group("path")
        candidate = Path(value)
        if not candidate.is_absolute():
            candidate = root / candidate
        resolved = candidate.resolve(strict=False)
        try:
            resolved.relative_to(root)
        except ValueError:
            continue
        if resolved not in seen:
            paths.append(resolved)
            seen.add(resolved)
    return paths


def expected_edit_artifact_paths(request: dict, task_payload: dict, agent_payload: dict) -> list[Path]:
    tools = {str(value or "").strip().lower().replace("-", "_") for value in agent_payload.get("tools") or []}
    if "edit_file" not in tools:
        return []
    description = str(task_payload.get("description") or "")
    if not re.search(r"(?i)\b(create|write|save|generate|implement|erstelle|schreib|speicher|erzeug)\w*\b", description):
        return []

    root = Path(str(request.get("cwd") or Path.cwd())).expanduser().resolve(strict=False)
    task_contract = "\n".join([description, str(task_payload.get("expectedOutput") or "")])
    raw_matches = [match.group("path") for match in EDIT_ARTIFACT_PATTERN.finditer(task_contract)]
    qualified_names = {
        Path(value).name.lower()
        for value in raw_matches
        if Path(value).parent != Path(".")
    }
    paths: list[Path] = []
    seen: set[Path] = set()
    for value in raw_matches:
        candidate = Path(value)
        if candidate.parent == Path(".") and candidate.name.lower() in qualified_names:
            continue
        if not candidate.is_absolute():
            candidate = root / candidate
        resolved = candidate.resolve(strict=False)
        try:
            resolved.relative_to(root)
        except ValueError:
            continue
        if resolved not in seen:
            paths.append(resolved)
            seen.add(resolved)
    return paths


def expected_required_artifact_paths(request: dict, task_payload: dict, agent_payload: dict) -> list[Path]:
    paths = expected_office_artifact_paths(request, task_payload, agent_payload)
    seen = set(paths)
    for path in expected_edit_artifact_paths(request, task_payload, agent_payload):
        if path not in seen:
            paths.append(path)
            seen.add(path)
    return paths


def missing_required_artifacts(request: dict, task_payload: dict, agent_payload: dict) -> list[Path]:
    return [
        path
        for path in expected_required_artifact_paths(request, task_payload, agent_payload)
        if not path.is_file() or path.stat().st_size <= 0
    ]


def build_artifact_repair_description(
    request: dict,
    task_payload: dict,
    agent_payload: dict,
    missing: list[Path],
    context_outputs: list[str],
) -> str:
    root = Path(str(request.get("cwd") or Path.cwd())).expanduser().resolve(strict=False)
    relative_paths = []
    for path in missing:
        try:
            relative_paths.append(path.relative_to(root).as_posix())
        except ValueError:
            relative_paths.append(path.name)
    context = "\n\n".join(context_outputs)
    tools = {str(value or "").strip().lower().replace("-", "_") for value in agent_payload.get("tools") or []}
    repair_contract = (
        "Immediately call office_workflow. Pass exactly output_path, title, and sections_json. "
        "sections_json must be a JSON array of objects with title plus body or bullets. "
        "Do not explain the schema, emit XML, or return a proposed call. The task succeeds only after the tool returns Created."
        if "office_workflow" in tools
        else "Immediately call edit_file for every missing path, then use read_file or bash to verify it. "
        "Do not return code, XML, JSON tool syntax, or a proposed call as plain text. The task succeeds only after the file exists."
    )
    return (
        f"{build_task_description(request, task_payload, agent_payload)}\n\n"
        "ARTIFACT REPAIR REQUIRED:\n"
        f"The previous attempt returned text but did not create: {', '.join(relative_paths)}.\n"
        + repair_contract
        + (f"\n\nUpstream results to use:\n{truncate_text(context, 12_000)}" if context else "")
    )


MARKDOWN_ARTIFACT_PATTERN = re.compile(
    r"(?i)(?P<path>(?:[A-Za-z]:[\\/])?[A-Za-z0-9_.-]+(?:[\\/][A-Za-z0-9_.-]+)*\.md)"
)


def deterministic_office_fallback(
    request: dict,
    task_payload: dict,
    agent_payload: dict,
    missing: list[Path],
) -> str | None:
    if not missing:
        return None
    tools = {str(value or "").strip().lower().replace("-", "_") for value in agent_payload.get("tools") or []}
    if "office_workflow" not in tools:
        return None

    root = Path(str(request.get("cwd") or Path.cwd())).expanduser().resolve(strict=False)
    description = str(task_payload.get("description") or "")
    source_path: Path | None = None
    for match in MARKDOWN_ARTIFACT_PATTERN.finditer(description):
        candidate = Path(match.group("path"))
        if not candidate.is_absolute():
            candidate = root / candidate
        resolved = candidate.resolve(strict=False)
        try:
            resolved.relative_to(root)
        except ValueError:
            continue
        if resolved.is_file():
            source_path = resolved
            break
    if source_path is None:
        return None

    sections: list[dict[str, object]] = []
    current: dict[str, object] | None = None
    for raw_line in source_path.read_text(encoding="utf-8", errors="replace").splitlines():
        line = raw_line.strip()
        heading = re.match(r"^#{1,3}\s+(.+)$", line)
        if heading:
            if current is not None:
                sections.append(current)
            current = {"title": heading.group(1).strip(), "bullets": []}
            continue
        if not line or current is None or line.startswith("```"):
            continue
        content = re.sub(r"^[-*+]\s+", "", line).strip()
        if content:
            bullets = current["bullets"]
            if isinstance(bullets, list) and len(bullets) < 12:
                bullets.append(content[:500])
    if current is not None:
        sections.append(current)
    sections = [section for section in sections if section.get("title")][:20]
    if not sections:
        return None

    title_match = re.search(r"(?i)\btitle\s+['\"]([^'\"]+)['\"]", description)
    title = title_match.group(1).strip() if title_match else str(request.get("name") or "Crew Artifact")
    from crew_tools import OfficeWorkflowTool

    results: list[str] = []
    for target in missing:
        relative = target.relative_to(root).as_posix()
        result = OfficeWorkflowTool(root)._run(relative, title, json.dumps(sections, ensure_ascii=False))
        if result.startswith("ERROR"):
            raise RuntimeError(result)
        results.append(result)
    return "\n".join(results)


def resolve_process(process_name: str):
    from crewai import Process  # type: ignore

    if process_name.lower() == "hierarchical":
        return Process.hierarchical
    return Process.sequential


def expected_office_artifact_paths(request: dict, task_payload: dict, agent_payload: dict) -> list[Path]:
    tools = {str(value or "").strip().lower().replace("-", "_") for value in agent_payload.get("tools") or []}
    if "office_workflow" not in tools:
        return []

    root = Path(str(request.get("cwd") or Path.cwd())).expanduser().resolve(strict=False)
    task_contract = "\n".join([
        str(task_payload.get("description") or ""),
        str(task_payload.get("expectedOutput") or ""),
    ])
    paths: list[Path] = []
    seen: set[Path] = set()
    for match in OFFICE_ARTIFACT_PATTERN.finditer(task_contract):
        value = match.group("path")
        candidate = Path(value)
        if not candidate.is_absolute():
            candidate = root / candidate
        resolved = candidate.resolve(strict=False)
        try:
            resolved.relative_to(root)
        except ValueError:
            continue
        if resolved not in seen:
            paths.append(resolved)
            seen.add(resolved)
    return paths


def expected_edit_artifact_paths(request: dict, task_payload: dict, agent_payload: dict) -> list[Path]:
    tools = {str(value or "").strip().lower().replace("-", "_") for value in agent_payload.get("tools") or []}
    if "edit_file" not in tools:
        return []
    description = str(task_payload.get("description") or "")
    if not re.search(r"(?i)\b(create|write|save|generate|implement|erstelle|schreib|speicher|erzeug)\w*\b", description):
        return []

    root = Path(str(request.get("cwd") or Path.cwd())).expanduser().resolve(strict=False)
    task_contract = "\n".join([description, str(task_payload.get("expectedOutput") or "")])
    raw_matches = [match.group("path") for match in EDIT_ARTIFACT_PATTERN.finditer(task_contract)]
    qualified_names = {
        Path(value).name.lower()
        for value in raw_matches
        if Path(value).parent != Path(".")
    }
    paths: list[Path] = []
    seen: set[Path] = set()
    for value in raw_matches:
        candidate = Path(value)
        if candidate.parent == Path(".") and candidate.name.lower() in qualified_names:
            continue
        if not candidate.is_absolute():
            candidate = root / candidate
        resolved = candidate.resolve(strict=False)
        try:
            resolved.relative_to(root)
        except ValueError:
            continue
        if resolved not in seen:
            paths.append(resolved)
            seen.add(resolved)
    return paths


def expected_required_artifact_paths(request: dict, task_payload: dict, agent_payload: dict) -> list[Path]:
    paths = expected_office_artifact_paths(request, task_payload, agent_payload)
    seen = set(paths)
    for path in expected_edit_artifact_paths(request, task_payload, agent_payload):
        if path not in seen:
            paths.append(path)
            seen.add(path)
    return paths


def missing_required_artifacts(request: dict, task_payload: dict, agent_payload: dict) -> list[Path]:
    return [
        path
        for path in expected_required_artifact_paths(request, task_payload, agent_payload)
        if not path.is_file() or path.stat().st_size <= 0
    ]


def build_artifact_repair_description(
    request: dict,
    task_payload: dict,
    agent_payload: dict,
    missing: list[Path],
    context_outputs: list[str],
) -> str:
    root = Path(str(request.get("cwd") or Path.cwd())).expanduser().resolve(strict=False)
    relative_paths = []
    for path in missing:
        try:
            relative_paths.append(path.relative_to(root).as_posix())
        except ValueError:
            relative_paths.append(path.name)
    context = "\n\n".join(context_outputs)
    tools = {str(value or "").strip().lower().replace("-", "_") for value in agent_payload.get("tools") or []}
    repair_contract = (
        "Immediately call office_workflow. Pass exactly output_path, title, and sections_json. "
        "sections_json must be a JSON array of objects with title plus body or bullets. "
        "Do not explain the schema, emit XML, or return a proposed call. The task succeeds only after the tool returns Created."
        if "office_workflow" in tools
        else "Immediately call edit_file for every missing path, then use read_file or bash to verify it. "
        "Do not return code, XML, JSON tool syntax, or a proposed call as plain text. The task succeeds only after the file exists."
    )
    return (
        f"{build_task_description(request, task_payload, agent_payload)}\n\n"
        "ARTIFACT REPAIR REQUIRED:\n"
        f"The previous attempt returned text but did not create: {', '.join(relative_paths)}.\n"
        + repair_contract
        + (f"\n\nUpstream results to use:\n{truncate_text(context, 12_000)}" if context else "")
    )


MARKDOWN_ARTIFACT_PATTERN = re.compile(
    r"(?i)(?P<path>(?:[A-Za-z]:[\\/])?[A-Za-z0-9_.-]+(?:[\\/][A-Za-z0-9_.-]+)*\.md)"
)


def deterministic_office_fallback(
    request: dict,
    task_payload: dict,
    agent_payload: dict,
    missing: list[Path],
) -> str | None:
    if not missing:
        return None
    tools = {str(value or "").strip().lower().replace("-", "_") for value in agent_payload.get("tools") or []}
    if "office_workflow" not in tools:
        return None

    root = Path(str(request.get("cwd") or Path.cwd())).expanduser().resolve(strict=False)
    description = str(task_payload.get("description") or "")
    source_path: Path | None = None
    for match in MARKDOWN_ARTIFACT_PATTERN.finditer(description):
        candidate = Path(match.group("path"))
        if not candidate.is_absolute():
            candidate = root / candidate
        resolved = candidate.resolve(strict=False)
        try:
            resolved.relative_to(root)
        except ValueError:
            continue
        if resolved.is_file():
            source_path = resolved
            break
    if source_path is None:
        return None

    sections: list[dict[str, object]] = []
    current: dict[str, object] | None = None
    for raw_line in source_path.read_text(encoding="utf-8", errors="replace").splitlines():
        line = raw_line.strip()
        heading = re.match(r"^#{1,3}\s+(.+)$", line)
        if heading:
            if current is not None:
                sections.append(current)
            current = {"title": heading.group(1).strip(), "bullets": []}
            continue
        if not line or current is None or line.startswith("```"):
            continue
        content = re.sub(r"^[-*+]\s+", "", line).strip()
        if content:
            bullets = current["bullets"]
            if isinstance(bullets, list) and len(bullets) < 12:
                bullets.append(content[:500])
    if current is not None:
        sections.append(current)
    sections = [section for section in sections if section.get("title")][:20]
    if not sections:
        return None

    title_match = re.search(r"(?i)\btitle\s+['\"]([^'\"]+)['\"]", description)
    title = title_match.group(1).strip() if title_match else str(request.get("name") or "Crew Artifact")
    from crew_tools import OfficeWorkflowTool

    results: list[str] = []
    for target in missing:
        relative = target.relative_to(root).as_posix()
        result = OfficeWorkflowTool(root)._run(relative, title, json.dumps(sections, ensure_ascii=False))
        if result.startswith("ERROR"):
            raise RuntimeError(result)
        results.append(result)
    return "\n".join(results)


def resolve_process(process_name: str):
    from crewai import Process  # type: ignore

    if process_name.lower() == "hierarchical":
        return Process.hierarchical
    return Process.sequential


def execute_definition(payload: dict) -> dict:
    crew_id = str(payload.get("id") or "crew-runtime")
    stream_id = str(payload.get("streamId") or "").strip() or None
    run_id = str(payload.get("runId") or "").strip() or None
    process_name = str(payload.get("process") or "sequential")
    agent_payloads = payload.get("agents") or []
    task_payloads = payload.get("tasks") or []
    if not isinstance(agent_payloads, list):
        agent_payloads = []
    if not isinstance(task_payloads, list):
        task_payloads = []

    task_payloads = order_task_payloads(task_payloads)

    validate_runtime_provider_models(payload, agent_payloads)
    openrouter_free = has_openrouter_free_agent(payload, agent_payloads)
    payload["_effectiveAgentMaxRpm"] = effective_max_rpm(payload, len(agent_payloads), openrouter_free)
    max_parallel_tasks = max(1, parse_int(payload.get("maxParallelTasks"), 1))
    if openrouter_free:
        max_parallel_tasks = min(max_parallel_tasks, 2)
    process_name, task_payloads = normalize_task_concurrency(
        task_payloads,
        process_name,
        max_parallel_tasks,
    )

    from crewai import Crew, Task  # type: ignore
    suppress_runtime_deprecation_warnings()

    agents_by_id = {
        str(agent_payload.get("id") or f"agent-{index}"): build_agent(payload, agent_payload)
        for index, agent_payload in enumerate(agent_payloads)
        if isinstance(agent_payload, dict)
    }

    task_specs = {
        str(task_payload.get("id") or f"task-{index}"): task_payload
        for index, task_payload in enumerate(task_payloads)
        if isinstance(task_payload, dict)
    }
    task_objects: dict[str, Task] = {}

    def resolve_task(task_id: str):
        if task_id in task_objects:
            return task_objects[task_id]

        task_payload = task_specs[task_id]
        agent_id = str(task_payload.get("agentId") or "")
        if agent_id not in agents_by_id:
            raise ValueError(f"Task {task_id} references unknown agent {agent_id}.")

        agent_payload = next(
            (
                candidate
                for candidate in agent_payloads
                if isinstance(candidate, dict) and str(candidate.get("id") or "").strip() == agent_id
            ),
            {},
        )

        context_refs = dedupe_task_refs([
            str(value)
            for value in [*(task_payload.get("context") or []), *(task_payload.get("dependencies") or [])]
            if str(value).strip() in task_specs
        ])
        context_tasks = [resolve_task(ref) for ref in context_refs]

        task_kwargs = {
            "description": build_task_description(payload, task_payload, agent_payload),
            "expected_output": str(task_payload.get("expectedOutput") or "").strip() or "Erstelle ein vollstaendiges Ergebnis.",
            "agent": agents_by_id[agent_id],
        }
        if context_tasks:
            task_kwargs["context"] = context_tasks
        if bool(task_payload.get("asyncExecution")) and process_name.lower() != "parallel":
            task_kwargs["async_execution"] = True

        task_obj = Task(**task_kwargs)
        task_objects[task_id] = task_obj
        return task_obj

    ordered_task_bindings: list[tuple[dict, Task]] = []
    for task_payload in task_payloads:
        if not isinstance(task_payload, dict):
            continue
        task_id = str(task_payload.get("id") or "")
        if not task_id:
            continue
        ordered_task_bindings.append((task_payload, resolve_task(task_id)))

    manager_agent = None
    manager_agent_id = str(payload.get("managerAgentId") or "").strip()
    if process_name.lower() == "hierarchical":
        if not manager_agent_id or manager_agent_id not in agents_by_id:
            raise ValueError("Hierarchical crew requires an active manager agent.")
        manager_agent = agents_by_id[manager_agent_id]
    crew_agents = [
        agent
        for agent_id, agent in agents_by_id.items()
        if manager_agent is None or agent_id != manager_agent_id
    ]
    if not crew_agents:
        raise ValueError("Crew requires at least one active non-manager agent.")

    crew_kwargs = {
        "agents": crew_agents,
        "tasks": [task_obj for _, task_obj in ordered_task_bindings],
        "process": resolve_process(process_name),
        "verbose": bool(payload.get("verbose")),
    }
    max_rpm = parse_int(payload.get("maxRpm"), 0)
    if openrouter_free and max_rpm > 0:
        max_rpm = min(max_rpm, 2)
    if max_rpm > 0:
        crew_kwargs["max_rpm"] = max_rpm
    if manager_agent is not None:
        crew_kwargs["manager_agent"] = manager_agent

    task_results: list[dict] = []
    logs: list[dict] = []
    status = "completed"
    error_message = None
    runtime_task_id = ordered_task_bindings[0][0].get("id") if ordered_task_bindings else "runtime"
    runtime_agent_id = manager_agent_id or "python-runtime"
    stdout_buffer = LiveCapture(
        logs,
        crew_id,
        runtime_agent_id,
        runtime_task_id,
        "runtime_stdout",
        stream_id,
        run_id,
    )
    stderr_buffer = LiveCapture(
        logs,
        crew_id,
        runtime_agent_id,
        runtime_task_id,
        "runtime_stderr",
        stream_id,
        run_id,
    )

    record_execution_log(
        logs,
        crew_id,
        runtime_agent_id,
        runtime_task_id,
        "runtime_context",
        summarize_runtime_context(payload),
        stream_id,
        run_id,
        agent_name="Runtime",
        phase="context",
        summary="Runtime context loaded",
        severity="info",
    )
    rate_limit_reason = str(payload.get("_rateLimitPolicyReason") or "").strip()
    if rate_limit_reason:
        record_execution_log(
            logs,
            crew_id,
            runtime_agent_id,
            runtime_task_id,
            "rate_limit_policy",
            rate_limit_reason,
            stream_id,
            run_id,
            agent_name="Runtime",
            phase="status",
            summary="Rate-limit protection active",
            severity="warning",
        )

    for agent_payload in agent_payloads:
        if not isinstance(agent_payload, dict):
            continue
        agent_id = str(agent_payload.get("id") or "runtime-agent")
        agent_name = agent_display_name(agent_payload, agent_id)
        provider = resolve_agent_provider(agent_payload)
        model = resolve_agent_model_label(payload, agent_payload)
        record_execution_log(
            logs,
            crew_id,
            agent_id,
            runtime_task_id,
            "agent_ready",
            " | ".join([
                f"Name: {agent_name}",
                f"Role: {str(agent_payload.get('role') or '-').strip()}",
                f"Provider: {provider}",
                f"Model: {model}",
                f"Model override: {str(agent_payload.get('modelOverride') or '-').strip() or '-'}",
                f"Delegation: {'allowed' if bool(agent_payload.get('allowDelegation')) else 'blocked'}",
                f"Tools: {format_value_list(agent_payload.get('tools') or [])}",
                f"MCP: {format_value_list(agent_payload.get('mcpServerNames') or [])}",
            ]),
            stream_id,
            run_id,
            agent_name=agent_name,
            provider=provider,
            model=model,
            phase="agent",
            summary=f"{agent_name} ready",
            severity="info",
        )

    for index, (task_payload, _) in enumerate(ordered_task_bindings):
        task_id = str(task_payload.get("id") or f"task-{index}")
        agent_id = str(task_payload.get("agentId") or "runtime-agent")
        agent_payload = find_agent_payload(agent_payloads, agent_id)
        agent_name = agent_display_name(agent_payload, agent_id)
        provider = resolve_agent_provider(agent_payload)
        model = resolve_agent_model_label(payload, agent_payload)
        task_title = task_display_title(task_payload, task_id)
        context_refs = dedupe_task_refs([
            str(value)
            for value in [*(task_payload.get("context") or []), *(task_payload.get("dependencies") or [])]
            if str(value).strip()
        ])
        handoff_detail = "\n".join([
            f"Task handed to agent: {agent_name}",
            f"Description: {truncate_text(task_payload.get('description') or '', 900)}",
            f"Expected output: {truncate_text(task_payload.get('expectedOutput') or '', 500)}",
            f"Context/dependencies: {', '.join(context_refs) if context_refs else '-'}",
            f"Async: {'yes' if bool(task_payload.get('asyncExecution')) else 'no'}",
            f"Technical agent ID: {agent_id}",
        ])
        record_execution_log(
            logs,
            crew_id,
            agent_id,
            task_id,
            "task_handoff",
            handoff_detail,
            stream_id,
            run_id,
            agent_name=agent_name,
            source_agent="Runtime",
            target_agent=agent_name,
            provider=provider,
            model=model,
            task_title=task_title,
            phase="handoff",
            summary=f"Runtime -> {agent_name}: {task_title}",
            detail=handoff_detail,
            severity="info",
        )
        thinking_detail = "\n".join([
            f"Work log: {agent_name} starts the task.",
            f"Task: {task_title}",
            f"Provider/model: {provider} / {model}",
        ])
        record_execution_log(
            logs,
            crew_id,
            agent_id,
            task_id,
            "thinking_phase",
            thinking_detail,
            stream_id,
            run_id,
            agent_name=agent_name,
            provider=provider,
            model=model,
            task_title=task_title,
            phase="thinking",
            summary=f"{agent_name} works on {task_title}",
            detail=thinking_detail,
            severity="info",
        )

    try:
        crew = Crew(**crew_kwargs)
        record_execution_log(
            logs,
            crew_id,
            runtime_agent_id,
            runtime_task_id,
            "crew_kickoff",
            f"CrewAI starts: process={process_name}, agents={len(crew_agents)}, tasks={len(ordered_task_bindings)}",
            stream_id,
            run_id,
            agent_name="Runtime",
            phase="status",
            summary="CrewAI started",
            severity="info",
        )
        with contextlib.redirect_stdout(stdout_buffer), contextlib.redirect_stderr(stderr_buffer):
            if process_name.lower() == "parallel":
                execute_parallel_crew(crew, ordered_task_bindings)
            else:
                crew.kickoff()
        stdout_buffer.flush()
        stderr_buffer.flush()
        record_execution_log(
            logs,
            crew_id,
            runtime_agent_id,
            runtime_task_id,
            "crew_finished",
            "CrewAI kickoff abgeschlossen.",
            stream_id,
            run_id,
            agent_name="Runtime",
            phase="status",
            summary="CrewAI abgeschlossen",
            severity="info",
        )
    except Exception as exc:
        status = "failed"
        error_message = classify_runtime_error(exc)
        stderr_buffer.write(traceback.format_exc())
        stderr_buffer.flush()
        record_execution_log(
            logs,
            crew_id,
            runtime_agent_id,
            runtime_task_id,
            "runtime_failed",
            error_message,
            stream_id,
            run_id,
            agent_name="Runtime",
            phase="error",
            summary="Runtime-Fehler",
            detail=traceback.format_exc(),
            severity="error",
        )

    repaired_outputs: dict[str, str] = {}
    artifact_retry_count = 1 if parse_int(payload.get("retryCount"), 0) > 0 else 0
    if status == "completed":
        for task_payload, _ in ordered_task_bindings:
            task_id = str(task_payload.get("id") or "runtime-task")
            agent_id = str(task_payload.get("agentId") or "runtime-agent")
            agent_payload = find_agent_payload(agent_payloads, agent_id)
            missing = missing_required_artifacts(payload, task_payload, agent_payload)
            if not missing:
                continue

            context_refs = dedupe_task_refs([
                str(value)
                for value in [*(task_payload.get("context") or []), *(task_payload.get("dependencies") or [])]
                if str(value).strip() in task_objects
            ])
            context_outputs = [
                output
                for ref in context_refs
                if (output := extract_task_output(task_objects[ref])) is not None
            ]
            agent_name = agent_display_name(agent_payload, agent_id)
            provider = resolve_agent_provider(agent_payload)
            model = resolve_agent_model_label(payload, agent_payload)
            task_title = task_display_title(task_payload, task_id)

            for attempt in range(1, artifact_retry_count + 1):
                record_execution_log(
                    logs,
                    crew_id,
                    agent_id,
                    task_id,
                    "artifact_repair",
                    f"Artifact repair attempt {attempt}/{artifact_retry_count}: "
                    + ", ".join(path.name for path in missing),
                    stream_id,
                    run_id,
                    agent_name=agent_name,
                    provider=provider,
                    model=model,
                    task_title=task_title,
                    phase="verification",
                    summary=f"Repair missing artifact for {task_title}",
                    severity="warning",
                )
                repair_agent = build_agent(payload, agent_payload)
                repair_task = Task(
                    description=build_artifact_repair_description(
                        payload,
                        task_payload,
                        agent_payload,
                        missing,
                        context_outputs,
                    ),
                    expected_output=str(task_payload.get("expectedOutput") or "").strip()
                    or "The exact path returned by office_workflow.",
                    agent=repair_agent,
                )
                repair_crew = Crew(
                    agents=[repair_agent],
                    tasks=[repair_task],
                    process=resolve_process("sequential"),
                    verbose=bool(payload.get("verbose")),
                )
                try:
                    with contextlib.redirect_stdout(stdout_buffer), contextlib.redirect_stderr(stderr_buffer):
                        repair_crew.kickoff()
                    stdout_buffer.flush()
                    stderr_buffer.flush()
                    repair_output = extract_task_output(repair_task)
                    if repair_output:
                        repaired_outputs[task_id] = repair_output
                except Exception as exc:
                    stderr_buffer.write(traceback.format_exc())
                    stderr_buffer.flush()
                    record_execution_log(
                        logs,
                        crew_id,
                        agent_id,
                        task_id,
                        "artifact_repair_failed",
                        classify_runtime_error(exc),
                        stream_id,
                        run_id,
                        agent_name=agent_name,
                        provider=provider,
                        model=model,
                        task_title=task_title,
                        phase="error",
                        summary=f"Artifact repair failed for {task_title}",
                        severity="error",
                    )
                missing = missing_required_artifacts(payload, task_payload, agent_payload)
                if not missing:
                    record_execution_log(
                        logs,
                        crew_id,
                        agent_id,
                        task_id,
                        "artifact_verified",
                        "Verified created artifact(s): " + ", ".join(
                            str(path) for path in expected_required_artifact_paths(payload, task_payload, agent_payload)
                        ),
                        stream_id,
                        run_id,
                        agent_name=agent_name,
                        provider=provider,
                        model=model,
                        task_title=task_title,
                        phase="verification",
                        summary=f"Artifact verified for {task_title}",
                        severity="info",
                    )
                    break

            if missing:
                try:
                    fallback_output = deterministic_office_fallback(
                        payload,
                        task_payload,
                        agent_payload,
                        missing,
                    )
                    if fallback_output:
                        repaired_outputs[task_id] = fallback_output
                        missing = missing_required_artifacts(payload, task_payload, agent_payload)
                        record_execution_log(
                            logs,
                            crew_id,
                            agent_id,
                            task_id,
                            "artifact_fallback",
                            fallback_output,
                            stream_id,
                            run_id,
                            agent_name=agent_name,
                            provider=provider,
                            model=model,
                            task_title=task_title,
                            phase="verification",
                            summary=f"Deterministic artifact fallback completed for {task_title}",
                            severity="warning",
                        )
                except Exception as exc:
                    record_execution_log(
                        logs,
                        crew_id,
                        agent_id,
                        task_id,
                        "artifact_fallback_failed",
                        classify_runtime_error(exc),
                        stream_id,
                        run_id,
                        agent_name=agent_name,
                        provider=provider,
                        model=model,
                        task_title=task_title,
                        phase="error",
                        summary=f"Artifact fallback failed for {task_title}",
                        severity="error",
                    )

            if missing:
                status = "failed"
                error_message = (
                    "ArtifactValidationError: The task returned without creating required artifact(s): "
                    + ", ".join(str(path) for path in missing)
                )
                record_execution_log(
                    logs,
                    crew_id,
                    agent_id,
                    task_id,
                    "artifact_missing",
                    error_message,
                    stream_id,
                    run_id,
                    agent_name=agent_name,
                    provider=provider,
                    model=model,
                    task_title=task_title,
                    phase="error",
                    summary=f"Required artifact missing for {task_title}",
                    severity="error",
                )

    for task_payload, task_obj in ordered_task_bindings:
        task_id = str(task_payload.get("id") or "runtime-task")
        agent_id = str(task_payload.get("agentId") or "runtime-agent")
        agent_payload = find_agent_payload(agent_payloads, agent_id)
        agent_name = agent_display_name(agent_payload, agent_id)
        provider = resolve_agent_provider(agent_payload)
        model = resolve_agent_model_label(payload, agent_payload)
        task_title = task_display_title(task_payload, task_id)
        output = repaired_outputs.get(task_id) or extract_task_output(task_obj)
        missing = missing_required_artifacts(payload, task_payload, agent_payload)
        task_status = "failed" if missing else ("completed" if output else ("failed" if status == "failed" else "completed"))
        task_results.append({
            "taskId": task_id,
            "agentId": agent_id,
            "status": task_status,
            "output": (
                error_message
                if missing
                else output if output is not None else (error_message if task_status == "failed" else None)
            ),
        })
        if output:
            record_execution_log(
                logs,
                crew_id,
                agent_id,
                task_id,
                "task_completed",
                output,
                stream_id,
                run_id,
                agent_name=agent_name,
                provider=provider,
                model=model,
                task_title=task_title,
                phase="result",
                summary=f"Task abgeschlossen: {task_title}",
                severity="info",
            )

    captured_stdout = stdout_buffer.getvalue().strip()
    captured_stderr = stderr_buffer.getvalue().strip()
    if captured_stdout and not any(entry.get("action") == "runtime_stdout" for entry in logs):
        record_execution_log(
            logs,
            crew_id,
            runtime_agent_id,
            ordered_task_bindings[-1][0].get("id") if ordered_task_bindings else "runtime",
            "runtime_stdout",
            captured_stdout,
            stream_id,
            run_id,
            agent_name="Runtime",
            phase="output",
            summary="Runtime-Ausgabe",
            severity="info",
        )
    if captured_stderr and not any(entry.get("action") == "runtime_stderr" for entry in logs):
        record_execution_log(
            logs,
            crew_id,
            runtime_agent_id,
            ordered_task_bindings[-1][0].get("id") if ordered_task_bindings else "runtime",
            "runtime_stderr",
            captured_stderr,
            stream_id,
            run_id,
            agent_name="Runtime",
            phase="error",
            summary="Runtime-Fehlerausgabe",
            severity="error",
        )

    if status == "failed" and error_message and not any(entry.get("action") == "runtime_stderr" for entry in logs):
        record_execution_log(
            logs,
            crew_id,
            runtime_agent_id,
            ordered_task_bindings[-1][0].get("id") if ordered_task_bindings else "runtime",
            "runtime_failed",
            error_message,
            stream_id,
            run_id,
            agent_name="Runtime",
            phase="error",
            summary="Runtime-Fehler",
            severity="error",
        )

    mark_recovered_provider_logs(logs, status)

    return {
        "crewId": crew_id,
        "status": status,
        "taskResults": task_results,
        "logs": logs,
        "error": error_message,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("command", choices=["status", "validate", "execute"])
    args = parser.parse_args()

    if args.command == "status":
        write_json_response(runtime_status())
        return 0

    if args.command == "validate":
        payload = read_payload()
        write_json_response(validate_definition(payload))
        return 0

    if args.command == "execute":
        payload = read_payload()
        try:
            write_json_response(execute_definition(payload))
        except Exception as exc:
            error_message = classify_runtime_error(exc)
            write_json_response({
                "crewId": str(payload.get("id") or "crew-runtime"),
                "status": "failed",
                "taskResults": [],
                "logs": [
                    make_execution_log(
                        str(payload.get("id") or "crew-runtime"),
                        str(payload.get("managerAgentId") or "python-runtime"),
                        "runtime",
                        "runtime_failed",
                        error_message,
                        agent_name="Runtime",
                        phase="error",
                        summary="Runtime-Fehler",
                        detail=traceback.format_exc(),
                        severity="error",
                    )
                ],
                "error": error_message,
            })
        return 0

    return 1


if __name__ == "__main__":
    raise SystemExit(main())
