"""Pure, service-free adversarial checks for the explicit resume fixture."""
import copy
import io
import hashlib
import json
import os
from pathlib import Path
import signal
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock
import uuid

import qualify_server_resume as subject


def identity():
    return str(uuid.uuid4())


class CursorTests(unittest.TestCase):
    def setUp(self):
        self.run = identity()
        self.values = subject.scenario_values(identity())
        self.lease = {"owner": identity(), "token": identity()}
        self.cursor = {"id": identity(), "sequence": 7, "origin": "server_agent_v1", "safe": True,
            "worker": self.lease["owner"], "lease": self.lease["token"], "attempt": identity(),
            "state": {"version": 1, "run_id": self.run, "safe_to_resume": True,
                "next_step": 2, "pending": None, "finished": None, "messages": [{"role": "tool",
                    "tool_call_id": self.values["call_id"], "content": "resume-fixture-ok"}]}}

    def test_true_completed_cursor_is_accepted_and_no_contents_are_returned_in_report(self):
        self.assertIs(subject.assert_completed_cursor(self.cursor, self.run, self.lease, self.values), self.cursor)

    def test_unsafe_untrusted_wrong_worker_lease_or_missing_attempt_never_authorize_resume(self):
        for key, value in [("safe", False), ("origin", "worker_other"), ("worker", identity()),
                           ("lease", identity()), ("attempt", None)]:
            with self.subTest(key=key):
                cursor = copy.deepcopy(self.cursor)
                cursor[key] = value
                with self.assertRaises(subject.QualificationFailure):
                    subject.assert_completed_cursor(cursor, self.run, self.lease, self.values)

    def test_before_tool_pending_finished_or_wrong_run_cursor_is_rejected(self):
        for key, value in [("version", 2), ("run_id", identity()), ("safe_to_resume", False),
                           ("next_step", 1), ("pending", {"next_call": 0}), ("finished", {})]:
            with self.subTest(key=key):
                cursor = copy.deepcopy(self.cursor)
                cursor["state"][key] = value
                with self.assertRaises(subject.QualificationFailure):
                    subject.assert_completed_cursor(cursor, self.run, self.lease, self.values)

    def test_missing_duplicate_foreign_or_error_tool_result_cannot_pass_as_completed_bash(self):
        good = self.cursor["state"]["messages"][0]
        for messages in [[], [good, good], [dict(good, role="assistant")],
                         [dict(good, tool_call_id="foreign")], [dict(good, content="tool failed")],
                         [dict(good, content=None)]]:
            with self.subTest(messages=messages):
                cursor = copy.deepcopy(self.cursor)
                cursor["state"]["messages"] = messages
                with self.assertRaises(subject.QualificationFailure):
                    subject.assert_completed_cursor(cursor, self.run, self.lease, self.values)

    def test_resumed_model_requires_actual_correlated_tool_result_exactly_once(self):
        good = self.cursor["state"]["messages"][0]
        subject.assert_resumed_context({"messages": [good]}, self.values)
        for messages in [[], [good, good], [dict(good, role="user")],
                         [dict(good, tool_call_id="other")], [dict(good, content=None)]]:
            with self.assertRaises(subject.QualificationFailure):
                subject.assert_resumed_context({"messages": messages}, self.values)


class FaultScopeTests(unittest.TestCase):
    def setUp(self):
        self.run = identity()
        self.lease = {"owner": identity(), "token": identity()}
        self.values = subject.scenario_values(identity())
        self.name = "resume_fault_" + uuid.uuid4().hex

    def test_fault_sql_binds_exact_run_worker_lease_attempt_tool_event_and_latest_provenance(self):
        sql = subject.fault_sql(self.run, self.lease, self.values, (11, 12), self.name)
        for value in [self.run, self.lease["owner"], self.lease["token"], self.values["call_id"],
                      "ORDER BY sequence DESC LIMIT 1", "latest.server_execution_attempt_id=NEW.server_execution_attempt_id",
                      "NEW.safe_to_resume=false", "latest.safe_to_resume=true", "kind='tool_completed'",
                      "BEFORE INSERT", "pg_advisory_xact_lock(11,12)"]:
            self.assertIn(value, sql)
        # SQL changes only own function/trigger schema; never checkpoint/state data.
        self.assertNotRegex(sql, r"(?i)\b(?:UPDATE|DELETE|INSERT\s+INTO)\b")

    def test_identifiers_and_keys_reject_injection_and_out_of_range_values(self):
        for name, key in [(self.name + ";DROP", (1, 2)), (self.name, (0, 1)),
                          (self.name, (1, 2 ** 31)), (self.name, (True, 1))]:
            with self.assertRaises(subject.QualificationFailure):
                subject.fault_sql(self.run, self.lease, self.values, key, name)
        values = dict(self.values, call_id="'; unsafe")
        with self.assertRaises(subject.QualificationFailure):
            subject.fault_sql(self.run, self.lease, values, (1, 2), self.name)

    def barrier(self, pids):
        queries = []
        fixture = SimpleNamespace(worker_application="exact_worker", sql=lambda sql: queries.append(sql) or json.dumps(pids))
        barrier = subject.AdvisoryBarrier(fixture, self.run, self.lease, self.values)
        return barrier, queries

    def test_waiter_query_requires_exact_key_own_database_application_and_real_transaction(self):
        barrier, queries = self.barrier([123])
        self.assertEqual(barrier.blocked(), 123)
        self.assertEqual(barrier.blocked_pid, 123)
        for value in ["NOT l.granted", "l.objsubid=2", "datname=current_database()",
                      "application_name='exact_worker'", "xact_start IS NOT NULL",
                      f"l.classid={barrier.key[0]}::oid", f"l.objid={barrier.key[1]}::oid"]:
            self.assertIn(value, queries[0])

    def test_no_waiter_is_not_a_fault_and_two_waiters_refuse_ambiguous_kill(self):
        barrier, _ = self.barrier([])
        self.assertIsNone(barrier.blocked())
        barrier, _ = self.barrier([123, 124])
        with self.assertRaises(subject.QualificationFailure):
            barrier.blocked()

    def test_disarm_unlocks_holder_before_schema_drop_and_unknown_exit_prevents_drop(self):
        barrier, _ = self.barrier([])
        events = []
        barrier.armed = True
        barrier.unlock = lambda: events.append("unlock")
        barrier.fixture.sql = lambda sql: events.append("drop")
        barrier.disarm()
        self.assertEqual(events, ["unlock", "drop"])
        barrier.armed = True
        barrier.unlock = mock.Mock(side_effect=subject.QualificationFailure("holder_unknown"))
        with self.assertRaises(subject.QualificationFailure):
            barrier.disarm()
        self.assertTrue(barrier.armed)
        self.assertEqual(events, ["unlock", "drop"])


class ModelAndReceiptTests(unittest.TestCase):
    def test_private_model_delivers_only_one_real_bash_append_with_bounded_timeout(self):
        values = subject.scenario_values(identity())
        result = subject.bash_message(values)
        self.assertEqual(len(result["tool_calls"]), 1)
        call = result["tool_calls"][0]
        self.assertEqual(call["function"]["name"], "Bash")
        arguments = json.loads(call["function"]["arguments"])
        self.assertEqual(arguments["timeout_seconds"], 10)
        self.assertIn(" >> /workspace/" + values["marker"], arguments["command"])
        self.assertEqual(arguments["command"].count(values["id"]), 1)
        with self.assertRaises(subject.QualificationFailure):
            subject.bash_message(dict(values, marker="../../foreign"))

    def test_receipt_binds_request_run_checkpoint_and_next_revision(self):
        run, request, checkpoint = identity(), identity(), identity()
        receipt = {"request_id": request, "run_id": run, "checkpoint_id": checkpoint, "queued_revision": 9}
        subject.assert_resume_receipt(receipt, run, request, checkpoint, 8)
        for field in receipt:
            wrong = dict(receipt, **{field: identity() if field != "queued_revision" else 10})
            with self.assertRaises(subject.QualificationFailure):
                subject.assert_resume_receipt(wrong, run, request, checkpoint, 8)
        with self.assertRaises(subject.QualificationFailure):
            subject.assert_resume_receipt(dict(receipt, extra=True), run, request, checkpoint, 8)

    def test_model_scenario_import_allocates_no_ports_threads_processes_or_docker(self):
        with mock.patch.object(subject.threading, "Thread", side_effect=AssertionError("no thread")), \
             mock.patch.object(subject.subprocess, "Popen", side_effect=AssertionError("no process")):
            scenario = subject.BashScenario()
        self.assertEqual(scenario.requests, 0)
        self.assertFalse(scenario.ready.is_set())
        self.assertFalse(scenario.resumed.is_set())


class RetentionAndInputsTests(unittest.TestCase):
    def arguments(self, directory):
        root = Path(directory).resolve()
        args = SimpleNamespace(execute=True, expected_sha256="a" * 64, expected_runner_sha256="b" * 64,
            expected_linux_docker_cli_sha256="c" * 64, expected_migration=59,
            server_bin=root / "server", runner_binary=root / "runner", linux_docker_cli=root / "docker-cli",
            runner_image_id=None, gui_image_id="sha256:" + "d" * 64, core_image_id="sha256:" + "e" * 64,
            docker_client=root / "docker.exe", docker_context="desktop-linux", baseline=root / "baseline.json",
            baseline_count=39, include_api_desktop_phase=False, report=root / "public.json")
        for path in [args.server_bin, args.runner_binary, args.linux_docker_cli, args.baseline]:
            path.write_bytes(b"private fixture bytes")
        return args

    def test_schema_native_sha_and_original39_inputs_are_mandatory(self):
        with tempfile.TemporaryDirectory() as directory:
            args = self.arguments(directory)
            accepted = subject.shared.qualification_inputs(args)
            self.assertEqual(accepted["runner_mode"], "native")
            for field, value in [("expected_sha256", ""), ("expected_runner_sha256", "f" * 63),
                                 ("expected_linux_docker_cli_sha256", None), ("baseline_count", 38),
                                 ("docker_context", "default")]:
                wrong = copy.copy(args)
                setattr(wrong, field, value)
                with self.assertRaises(subject.QualificationFailure):
                    subject.shared.qualification_inputs(wrong)

    def test_execute_and_schema59_gate_before_helper_import_or_parent_mutation(self):
        with tempfile.TemporaryDirectory() as directory:
            for execute, migration, check in [(False, 59, "explicit_real_resume_fixture_execution_required"),
                                               (True, 58, "resume_schema59_or_later_required")]:
                args = self.arguments(directory)
                args.execute, args.expected_migration = execute, migration
                with mock.patch.object(subject, "parser", return_value=SimpleNamespace(parse_args=lambda: args)), \
                     mock.patch.object(subject.shared, "load_runner_helpers") as load, \
                     mock.patch.object(subject.shared, "QualificationRoot") as create, \
                     mock.patch("builtins.print"):
                    self.assertEqual(subject.main(), 1)
                load.assert_not_called()
                create.assert_not_called()
                self.assertEqual(json.loads(args.report.read_text())["failure_check"], check)

    def main_without_services(self, directory, *, interrupt=False, cleanup_fail=False):
        args = self.arguments(directory)
        root_parent = Path(directory) / "private"
        marker_writes = []
        def private_root(parent):
            parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            path = parent / identity()
            path.mkdir(mode=0o700)
            return path
        def write_private(path, data):
            with os.fdopen(os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "wb") as destination:
                destination.write(data.encode() if isinstance(data, str) else data)
            Path(path).chmod(0o600)
            if Path(path).name == "fixture.json":
                marker_writes.append(json.loads(data))
        helpers = SimpleNamespace(private_root=private_root, write_private=write_private, LAST_ACTION=None)
        parent = SimpleNamespace(assert_preserved=lambda: None, owned_inventory=lambda scope: {})
        helpers.ParentDocker = lambda **unused: parent
        def runner_factory(parent_arg, root, **unused):
            result = SimpleNamespace(root=root, native_binary=True, identity_record={"scope": root.parent.name},
                protocol=object(), ledger=lambda: [], start=lambda: None)
            result.cleanup = mock.Mock(side_effect=subject.QualificationFailure("unknown_physical_cleanup") if cleanup_fail else None)
            return result
        helpers.RunnerFixture = runner_factory
        def fixture_factory(root, *unused, **keywords):
            return SimpleNamespace(root=root, children=[], data=root / "database", cleanup=lambda: None,
                last_phase="injected_pure_phase", last_api_status=None, failed_api_status=None,
                model=None, observer=None, barrier=None)
        def qualify(fixture, output, migration):
            output["checks"].append({"name": "pure_stub_only", "passed": True})
            if interrupt:
                signal.raise_signal(signal.SIGTERM)
        original_root = subject.shared.QualificationRoot
        with mock.patch.object(subject, "parser", return_value=SimpleNamespace(parse_args=lambda: args)), \
             mock.patch.object(subject.os, "name", "posix"), \
             mock.patch.object(subject.os, "geteuid", return_value=getattr(os, "geteuid", lambda: 1000)()), \
             mock.patch.object(subject, "checked_server_elf", side_effect=lambda path, *unused: path), \
             mock.patch.object(subject.shared, "validate_native_inputs"), \
             mock.patch.object(subject.shared, "owned_native_copy", side_effect=lambda source, destination, *unused: shutil_copy(source, destination)), \
             mock.patch.object(subject, "owned_server_copy", side_effect=lambda source, destination, *unused: shutil_copy(source, destination)), \
             mock.patch.object(subject.shared, "load_runner_helpers", return_value=(helpers, args.runner_binary)), \
             mock.patch.object(subject.shared, "QualificationRoot", side_effect=lambda supplied, **keywords: original_root(supplied, parent=root_parent, **keywords)), \
             mock.patch.object(subject, "ResumeFixture", side_effect=fixture_factory), \
             mock.patch.object(subject, "qualify", side_effect=qualify), \
             mock.patch.object(subject.shutil, "which", return_value="owned-openssl"), \
             mock.patch.object(Path, "is_file", autospec=True, side_effect=lambda path: True if path == subject.PG_BIN / "initdb" else original_is_file(path)), \
             mock.patch("builtins.print"):
            code = subject.main()
        report = json.loads(args.report.read_text())
        self.assertEqual(len(marker_writes), 1)
        self.assertEqual(marker_writes[0]["kind"], "server_resume")
        self.assertEqual(marker_writes[0], report["fixture_identity"])
        return code, report, root_parent

    @unittest.skipUnless(os.name == "posix" and os.geteuid() != 0, "real owner-only POSIX root assertion")
    def test_catchable_sigterm_retains_root_after_cleanup_and_restores_signal_handler(self):
        prior = signal.getsignal(signal.SIGTERM)
        with tempfile.TemporaryDirectory() as directory:
            code, report, parent = self.main_without_services(directory, interrupt=True)
            self.assertEqual(code, 1)
            self.assertFalse(report["passed"])
            root = Path(report["fixture_root"])
            self.assertEqual(root.parent, parent)
            self.assertTrue((root / "fixture.json").is_file())
            self.assertTrue((root / "evidence/ledger-before-cleanup.json").is_file())
            self.assertEqual(report["private_root_disposition"], "retained_failed_qualification")
            self.assertEqual(signal.getsignal(signal.SIGTERM), prior)

    @unittest.skipUnless(os.name == "posix" and os.geteuid() != 0, "real owner-only POSIX root assertion")
    def test_success_removes_only_exact_root_after_all_physical_cleanup_proofs(self):
        with tempfile.TemporaryDirectory() as directory:
            code, report, parent = self.main_without_services(directory)
            self.assertEqual(code, 0)
            self.assertTrue(report["passed"])
            self.assertEqual(set(report["cleanup_proofs"]), subject.shared.ROOT_CLEANUP_KEYS)
            self.assertTrue(all(report["cleanup_proofs"].values()))
            self.assertFalse(Path(report["fixture_root"]).exists())
            self.assertTrue(parent.exists())

    @unittest.skipUnless(os.name == "posix" and os.geteuid() != 0, "real owner-only POSIX root assertion")
    def test_unknown_runner_cleanup_never_reports_success_or_deletes_root(self):
        with tempfile.TemporaryDirectory() as directory:
            code, report, _ = self.main_without_services(directory, cleanup_fail=True)
            self.assertEqual(code, 1)
            self.assertFalse(report["passed"])
            self.assertFalse(report["cleanup_proofs"]["runner"])
            self.assertTrue(Path(report["fixture_root"]).exists())
            self.assertEqual(report["private_root_disposition"], "retained_failed_qualification")


original_is_file = Path.is_file


class RootIdentityTests(unittest.TestCase):
    def test_typed_resume_marker_is_created_once_and_cannot_be_rewritten_or_retyped_for_removal(self):
        with tempfile.TemporaryDirectory() as directory:
            parent = Path(directory).resolve()
            writes = []
            def private_root(root):
                path = root / identity()
                path.mkdir(mode=0o700)
                return path
            def write_private(path, data):
                with os.fdopen(os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "wb") as output:
                    output.write(data.encode())
                writes.append(Path(path))
            helpers = SimpleNamespace(private_root=private_root, write_private=write_private)
            storage = subject.shared.QualificationRoot(helpers, parent=parent, kind="server_resume")
            marker = storage.path / "fixture.json"
            original = marker.read_bytes()
            self.assertEqual(writes, [marker])
            self.assertEqual(json.loads(original), storage.identity)
            self.assertEqual(storage.identity["kind"], "server_resume")
            with self.assertRaises(FileExistsError):
                helpers.write_private(marker, json.dumps(storage.identity))
            storage.identity["kind"] = "linux_desktop_ha"
            with self.assertRaises(subject.QualificationFailure):
                storage.finish(True, {key: True for key in subject.shared.ROOT_CLEANUP_KEYS})
            self.assertTrue(storage.path.is_dir())
            self.assertEqual(marker.read_bytes(), original)

    def test_unknown_or_untyped_kind_fails_before_private_root_or_marker_creation(self):
        helpers = SimpleNamespace(private_root=mock.Mock(), write_private=mock.Mock())
        for kind in [None, "", "other_fixture", "server_resume/..", [], {}]:
            with self.subTest(kind=kind):
                with self.assertRaises(subject.QualificationFailure):
                    subject.shared.QualificationRoot(helpers, kind=kind)
        helpers.private_root.assert_not_called()
        helpers.write_private.assert_not_called()

    def test_prestart_failure_reports_only_allowed_source_names_functions_and_numeric_lines(self):
        with tempfile.TemporaryDirectory() as directory:
            args = RetentionAndInputsTests().arguments(directory)
            args.execute = False
            with mock.patch.object(subject, "parser", return_value=SimpleNamespace(parse_args=lambda: args)), \
                 mock.patch.object(subject.shared, "load_runner_helpers") as load, \
                 mock.patch("builtins.print"):
                self.assertEqual(subject.main(), 1)
            load.assert_not_called()
            report = json.loads(args.report.read_text())
            positions = report["failure_source_positions"]
            self.assertTrue(any(position["source"] == "qualify_server_resume.py"
                                and position["function"] == "main" for position in positions))
            self.assertTrue(positions)
            for position in positions:
                self.assertEqual(set(position), {"source", "function", "line"})
                self.assertEqual(Path(position["source"]).name, position["source"])
                self.assertIn(position["source"], {"qualify_server_resume.py", "qualify_runtime.py"})
                self.assertIs(type(position["line"]), int)
                self.assertGreater(position["line"], 0)


class NativeServerValidationTests(unittest.TestCase):
    def elf(self, parent, name="candidate"):
        path = Path(parent).resolve() / name
        path.write_bytes(b"\x7fELFowned source-only fixture")
        path.chmod(0o700)
        return path, hashlib.sha256(path.read_bytes()).hexdigest()

    def test_exact_elf_sha_required_and_relative_nonelf_or_wrong_digest_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            path, digest = self.elf(directory)
            self.assertEqual(subject.checked_server_elf(path, digest, "binary_gate"), path)
            for candidate, expected in [(Path(path.name), digest), (path, "a" * 64)]:
                with self.assertRaises(subject.QualificationFailure):
                    subject.checked_server_elf(candidate, expected, "binary_gate")
            path.write_bytes(b"not an ELF")
            with self.assertRaises(subject.QualificationFailure):
                subject.checked_server_elf(path, hashlib.sha256(path.read_bytes()).hexdigest(), "binary_gate")

    def test_fixed_server512mib_bound_does_not_change_runner_or_cli120mib_gate(self):
        with tempfile.TemporaryDirectory() as directory:
            path, _ = self.elf(directory)
            # A sparse test-only ELF crosses the old gate without executing it.
            with path.open("r+b") as source:
                source.truncate(120 * 1024 * 1024 + 1)
            digest = subject.file_sha256(path)
            self.assertEqual(subject.checked_server_elf(path, digest, "server_gate"), path)
            with self.assertRaises(subject.QualificationFailure):
                subject.shared.checked_native_elf(path, digest, "runner_cli_gate")
            with path.open("r+b") as source:
                source.truncate(subject.MAX_SERVER_BINARY_BYTES + 1)
            with self.assertRaises(subject.QualificationFailure):
                subject.checked_server_elf(path, digest, "server_gate")

    @unittest.skipUnless(os.name == "posix", "actual POSIX owner and executable mode")
    def test_actual_executable_and_uid_are_required(self):
        with tempfile.TemporaryDirectory() as directory:
            path, digest = self.elf(directory)
            path.chmod(0o600)
            with self.assertRaises(subject.QualificationFailure):
                subject.checked_server_elf(path, digest, "binary_gate")
            path.chmod(0o700)
            with mock.patch.object(subject.os, "geteuid", return_value=os.geteuid() + 1):
                with self.assertRaises(subject.QualificationFailure):
                    subject.checked_server_elf(path, digest, "binary_gate")

    @unittest.skipUnless(os.name == "posix", "actual POSIX symlink refusal")
    def test_leaf_and_parent_symlinks_are_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path, digest = self.elf(directory)
            leaf = root / "leaf-link"
            leaf.symlink_to(path)
            with self.assertRaises(subject.QualificationFailure):
                subject.checked_server_elf(leaf, digest, "binary_gate")
            child = root / "private"
            child.mkdir(mode=0o700)
            inner, digest = self.elf(child)
            parent = root / "parent-link"
            parent.symlink_to(child, target_is_directory=True)
            with self.assertRaises(subject.QualificationFailure):
                subject.checked_server_elf(parent / inner.name, digest, "binary_gate")

    @unittest.skipUnless(os.name == "posix", "actual private POSIX copy mode and ownership")
    def test_private_copy_is_exact0500_owned_exclusive_and_changed_bytes_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            root.chmod(0o700)
            path, digest = self.elf(root)
            copy_path = root / "approved-server"
            self.assertEqual(subject.owned_server_copy(path, copy_path, digest, "private_copy_gate"), copy_path)
            self.assertEqual(copy_path.read_bytes(), path.read_bytes())
            self.assertEqual(copy_path.stat().st_mode & 0o777, 0o500)
            self.assertEqual(copy_path.stat().st_uid, os.geteuid())
            with self.assertRaises(FileExistsError):
                subject.owned_server_copy(path, copy_path, digest, "private_copy_gate")
            copy_path.chmod(0o700)
            copy_path.write_bytes(b"\x7fELFchanged")
            with self.assertRaises(subject.QualificationFailure):
                subject.checked_server_elf(copy_path, digest, "private_copy_gate")

    @unittest.skipUnless(os.name == "posix", "actual private POSIX parent gate")
    def test_nonprivate_or_foreign_copy_parent_refuses_before_file_creation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path, digest = self.elf(root)
            root.chmod(0o755)
            destination = root / "approved-server"
            with self.assertRaises(subject.QualificationFailure):
                subject.owned_server_copy(path, destination, digest, "private_copy_gate")
            self.assertFalse(destination.exists())

    def test_replaced_descriptor_or_mutated_source_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            path, digest = self.elf(directory)
            actual = os.fstat
            count = 0
            def changing_stat(fd):
                nonlocal count
                count += 1
                value = actual(fd)
                if count == 2:
                    result = SimpleNamespace(**{field: getattr(value, field) for field in [
                        "st_dev", "st_ino", "st_size", "st_mtime_ns", "st_ctime_ns", "st_mode", "st_uid"]})
                    result.st_mtime_ns += 1
                    return result
                return value
            with mock.patch.object(subject.os, "fstat", side_effect=changing_stat):
                with self.assertRaises(subject.QualificationFailure):
                    subject.checked_server_elf(path, digest, "binary_gate")


def shutil_copy(source, destination):
    import shutil
    shutil.copyfile(source, destination)
    return destination


if __name__ == "__main__":
    unittest.main()
