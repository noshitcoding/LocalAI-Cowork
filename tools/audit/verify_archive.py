#!/usr/bin/env python3
"""Offline Ed25519 audit verification; a separately trusted public-key pin is required."""
import argparse
import base64
import hashlib
import json
import re
import sys
import uuid
from pathlib import Path

MAX_BYTES = 16 * 1024 * 1024
ZERO = "0" * 64


def canonical(value):
    return json.dumps(value, ensure_ascii=False, allow_nan=False, separators=(",", ":")).encode("utf-8")


def exact(value, names):
    if not isinstance(value, dict) or set(value) != set(names):
        raise ValueError("invalid_fields")


def number(value):
    if isinstance(value, bool) or not isinstance(value, int) or not 0 <= value <= 9_007_199_254_740_991:
        raise ValueError("invalid_number")
    return value


def identity(value):
    if not isinstance(value, str) or str(uuid.UUID(value)) != value or uuid.UUID(value).int == 0:
        raise ValueError("invalid_id")
    return value


def digest(value):
    if not isinstance(value, str) or not re.fullmatch("[0-9a-f]{64}", value):
        raise ValueError("invalid_digest")
    return value


def decode(value, count):
    if not isinstance(value, str) or not re.fullmatch("[A-Za-z0-9_-]+", value):
        raise ValueError("invalid_base64")
    result = base64.b64decode(value.replace("-", "+").replace("_", "/") + "=" * (-len(value) % 4), validate=True)
    if len(result) != count:
        raise ValueError("invalid_key_or_signature")
    return result


def verify(value, pin):
    exact(value, ["format", "request_id", "captured_at", "public_key", "public_key_sha256", "page", "signature"])
    if value["format"] != "cowork.audit.signed.v1" or digest(value["public_key_sha256"]) != pin:
        raise ValueError("unexpected_key_or_format")
    identity(value["request_id"])
    if number(value["captured_at"]) == 0:
        raise ValueError("invalid_time")
    public = decode(value["public_key"], 32)
    if hashlib.sha256(public).hexdigest() != pin:
        raise ValueError("unexpected_key")
    page = value["page"]
    names = ["format", "scope", "chain_id", "after_sequence", "previous_sha256", "through_sequence", "through_sha256", "events", "next_sequence", "complete"]
    exact(page, names)
    if page["format"] != "cowork.audit.page.v1" or not isinstance(page["scope"], str):
        raise ValueError("invalid_page")
    if page["scope"] != "personal":
        kind, sep, owner = page["scope"].partition(":")
        if not sep or kind not in ("account", "organization"):
            raise ValueError("invalid_scope")
        identity(owner)
    identity(page["chain_id"])
    sequence = number(page["after_sequence"])
    through = number(page["through_sequence"])
    previous = digest(page["previous_sha256"])
    digest(page["through_sha256"])
    if through < sequence or sequence == 0 and previous != ZERO or not isinstance(page["events"], list) or len(page["events"]) > 500:
        raise ValueError("invalid_page")
    events = []
    event_names = ["sequence", "event_id", "action", "actor_id", "target_id", "observed_at", "origin", "source_sha256", "previous_sha256", "sha256"]
    for event in page["events"]:
        exact(event, event_names)
        sequence += 1
        if number(event["sequence"]) != sequence or event["previous_sha256"] != previous or number(event["observed_at"]) == 0:
            raise ValueError("chain_sequence_or_time")
        if not isinstance(event["event_id"], str) or not 1 <= len(event["event_id"]) <= 256 or any(ord(c) < 32 for c in event["event_id"]):
            raise ValueError("invalid_event")
        if not isinstance(event["action"], str) or not re.fullmatch("[a-z0-9_.:-]{1,100}", event["action"]):
            raise ValueError("invalid_action")
        if event["origin"] not in ("native_v2", "legacy_unsigned", "preserved_archive"):
            raise ValueError("invalid_origin")
        for field in ("actor_id", "target_id"):
            if event[field] is not None:
                identity(event[field])
        if event["source_sha256"] is not None:
            digest(event["source_sha256"])
        expected = hashlib.sha256(canonical(["cowork:audit:event:v1", page["scope"], page["chain_id"], sequence,
            event["event_id"], event["action"], event["actor_id"], event["target_id"], event["observed_at"],
            event["origin"], event["source_sha256"], previous])).hexdigest()
        if digest(event["sha256"]) != expected:
            raise ValueError("chain_digest")
        previous = expected
        events.append({key: event[key] for key in event_names})
    if number(page["next_sequence"]) != sequence or sequence > through or type(page["complete"]) is not bool or page["complete"] != (sequence == through):
        raise ValueError("invalid_boundary")
    if page["complete"] and previous != page["through_sha256"] or not page["complete"] and not events:
        raise ValueError("incomplete_chain")
    normalized = {name: events if name == "events" else page[name] for name in names}
    signed = canonical(["cowork:audit:signature:v1", value["format"], value["request_id"], value["captured_at"], value["public_key"], pin, normalized])
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey
    Ed25519PublicKey.from_public_bytes(public).verify(decode(value["signature"], 64), signed)
    return normalized


def verify_metadata(value, pin, domain, unsigned):
    public = decode(value["public_key"], 32)
    if value["public_key_sha256"] != pin or hashlib.sha256(public).hexdigest() != pin:
        raise ValueError("unexpected_key")
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey
    Ed25519PublicKey.from_public_bytes(public).verify(
        decode(value["signature"], 64), canonical([domain, value["public_key"], pin, unsigned]))


def verify_proof(value, pin, original_snapshot=None):
    """Verify native assessment/retirement receipts without exporting an HMAC key."""
    kind = value.get("format") if isinstance(value, dict) else None
    if kind == "cowork.audit.v1-hmac.verification.v1":
        names = ["format", "request_id", "scope", "captured_at", "archives", "assessment",
                 "key_source", "public_key", "public_key_sha256", "signature"]
        exact(value, names)
        if value["scope"] != "personal" or value["key_source"] != "windows-credential-manager-v1-read-only" or original_snapshot is not None:
            raise ValueError("invalid_native_assessment")
        archives = value["archives"]
        if not isinstance(archives, list) or not 1 <= len(archives) <= 4:
            raise ValueError("invalid_original_selection")
        unique = set()
        originals = []
        for archive in archives:
            exact(archive, ["request_id", "content_sha256"])
            if identity(archive["request_id"]) in unique:
                raise ValueError("duplicate_original")
            unique.add(archive["request_id"])
            digest(archive["content_sha256"])
            originals.append({"request_id": archive["request_id"], "content_sha256": archive["content_sha256"]})
        assessment = value["assessment"]
        fields = ["valid", "signed_events", "legacy_events", "first_sequence", "last_sequence",
                  "chain_complete", "first_problem_line", "error_code"]
        exact(assessment, fields)
        for field in ("valid", "chain_complete"):
            if type(assessment[field]) is not bool:
                raise ValueError("invalid_assessment_flag")
        for field in ("signed_events", "legacy_events"):
            if number(assessment[field]) > 100_000:
                raise ValueError("assessment_limit")
        for field in ("first_sequence", "last_sequence"):
            sequence = assessment[field]
            if sequence is not None and (not isinstance(sequence, str) or not re.fullmatch("[1-9][0-9]{0,19}", sequence) or int(sequence) > 2**64-1):
                raise ValueError("invalid_v1_sequence")
        if assessment["first_problem_line"] is not None and number(assessment["first_problem_line"]) == 0:
            raise ValueError("invalid_problem_line")
        if assessment["error_code"] is not None and (not isinstance(assessment["error_code"], str) or not re.fullmatch("[a-z0-9_]{1,100}", assessment["error_code"])):
            raise ValueError("invalid_assessment_code")
        unsigned = {name: originals if name == "archives" else {field: assessment[field] for field in fields} if name == "assessment" else "" if name in ("public_key", "public_key_sha256", "signature") else value[name] for name in names}
        domain = "cowork:audit:v1-hmac-verification:v1"
        report = {"proof_kind": "native_v1_assessment", "selected_history_valid": assessment["valid"],
                  "selected_chain_complete": assessment["chain_complete"], "signed_events": assessment["signed_events"],
                  "legacy_events": assessment["legacy_events"], "originals": len(archives)}
    elif kind == "cowork.audit.retirement.v1":
        names = ["format", "request_id", "scope", "captured_at", "original", "new_chain_id",
                 "actor_id", "public_key", "public_key_sha256", "signature"]
        exact(value, names)
        identity(value["new_chain_id"])
        if value["actor_id"] is not None:
            identity(value["actor_id"])
        original = value["original"]
        fields = ["scope", "snapshot_sha256", "prior_chain_id", "prior_sequence", "records", "bytes",
                  "corrupt", "verification_complete", "public_key_sha256"]
        exact(original, fields)
        if original["scope"] != value["scope"] or original["public_key_sha256"] != pin or original["corrupt"] is not True or type(original["verification_complete"]) is not bool:
            raise ValueError("invalid_retirement_origin")
        digest(original["snapshot_sha256"])
        if original["prior_chain_id"] is not None:
            identity(original["prior_chain_id"])
        sequence = original["prior_sequence"]
        if not isinstance(sequence, str) or not re.fullmatch("-?[0-9]{1,19}", sequence) or not -(2**63) <= int(sequence) < 2**63:
            raise ValueError("invalid_original_sequence")
        if value["new_chain_id"] == original["prior_chain_id"] or number(original["records"]) > 100_000 or not 0 < number(original["bytes"]) <= 64*1024*1024:
            raise ValueError("invalid_original_bounds")
        if original_snapshot is not None:
            with original_snapshot.open("rb") as source:
                raw = source.read(64*1024*1024+1)
            if len(raw) != original["bytes"] or hashlib.sha256(raw).hexdigest() != original["snapshot_sha256"]:
                raise ValueError("original_snapshot_integrity")
        unsigned = {name: {field: original[field] for field in fields} if name == "original" else "" if name in ("public_key", "public_key_sha256", "signature") else value[name] for name in names}
        domain = "cowork:audit:retirement:v1"
        report = {"proof_kind": "corrupt_chain_retirement", "original_chain_was_corrupt": True,
                  "original_snapshot_sha256": original["snapshot_sha256"], "original_bytes_checked": original_snapshot is not None,
                  "new_chain_id": value["new_chain_id"]}
    else:
        raise ValueError("unsupported_proof")
    identity(value["request_id"])
    if number(value["captured_at"]) == 0 or not isinstance(value["scope"], str):
        raise ValueError("invalid_proof_metadata")
    if value["scope"] != "personal":
        prefix, separator, owner = value["scope"].partition(":")
        if not separator or prefix not in ("account", "organization"):
            raise ValueError("invalid_scope")
        identity(owner)
    verify_metadata(value, pin, domain, unsigned)
    return {"valid": True, "scope": value["scope"], "request_id": value["request_id"],
            "public_key_sha256": pin, **report}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path)
    parser.add_argument("--public-key-sha256", required=True)
    parser.add_argument("--original-snapshot", type=Path, help="Explicit original SQL snapshot for a signed retirement receipt")
    args = parser.parse_args()
    try:
        pin = digest(args.public_key_sha256)
        with args.archive.open("rb") as source:
            data = source.read(MAX_BYTES + 1)
        if len(data) > MAX_BYTES:
            raise ValueError("archive_size")
        value = json.loads(data)
        if isinstance(value, dict) and value.get("format") in ("cowork.audit.v1-hmac.verification.v1", "cowork.audit.retirement.v1"):
            print(json.dumps(verify_proof(value, pin, args.original_snapshot)))
            return 0
        if args.original_snapshot is not None:
            raise ValueError("original_snapshot_requires_retirement_receipt")
        if isinstance(value, dict) and value.get("format") == "cowork.audit.bundle.v1":
            exact(value, ["format", "pages"])
            if not isinstance(value["pages"], list) or not 1 <= len(value["pages"]) <= 200:
                raise ValueError("bundle_size")
            pages = [verify(v, pin) for v in value["pages"]]
        else:
            pages = [verify(value, pin)]
        first = pages[0]
        after, previous = first["after_sequence"], first["previous_sha256"]
        for page in pages:
            if any(page[k] != first[k] for k in ("scope", "chain_id", "through_sequence", "through_sha256")) or page["after_sequence"] != after or page["previous_sha256"] != previous:
                raise ValueError("bundle_continuity")
            after = page["next_sequence"]
            if page["events"]:
                previous = page["events"][-1]["sha256"]
        print(json.dumps({"valid": True, "scope": first["scope"], "chain_id": first["chain_id"], "pages": len(pages),
            "after_sequence": first["after_sequence"], "next_sequence": after, "through_sequence": first["through_sequence"], "complete": pages[-1]["complete"], "public_key_sha256": pin}))
        return 0
    except ImportError:
        print('{"valid":false,"error":"cryptography_dependency_unavailable"}', file=sys.stderr)
    except Exception:
        print('{"valid":false,"error":"archive_verification_failed"}', file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
