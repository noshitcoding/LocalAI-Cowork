"""Dependency-free regression coverage for the installed-runtime failure fixes."""
import unittest
import time
import main as runtime


class ReadinessRegressions(unittest.TestCase):
    def test_manager_access_is_removed_only_from_the_execution_copy(self):
        stored = {"id": "manager", "tools": ["web_search"], "mcpServerNames": ["local"]}
        effective = runtime.effective_agent_for_execution({"managerAgentId": "manager"}, stored, "hierarchical")
        self.assertEqual(effective["tools"], [])
        self.assertEqual(effective["mcpServerNames"], [])
        self.assertEqual(stored["tools"], ["web_search"])
        self.assertIs(runtime.effective_agent_for_execution({"managerAgentId": "other"}, stored, "hierarchical"), stored)

    def test_free_router_retains_its_model_namespace_after_litellm_routing(self):
        self.assertEqual(runtime.normalize_model_name("openrouter", "openrouter/free"), "openrouter/openrouter/free")
        self.assertTrue(runtime.is_openrouter_free_model("openrouter/free"))
        self.assertTrue(runtime.is_openrouter_free_model("openrouter/openrouter/free"))

    def test_openrouter_model_is_not_replaced_by_the_only_catalog_entry(self):
        agent = {"id": "worker", "providerKind": "openrouter"}
        payload = {"providerConfigs": {"openRouter": {"model": "retired/model:free", "models": ["openrouter/free"], "modelCatalogCheckedAt": time.time() * 1000, "apiKey": "test-fixture"}}}
        self.assertEqual(runtime.resolve_agent_model_label(payload, agent), "retired/model:free")
        with self.assertRaisesRegex(ValueError, "OpenRouter model unavailable"):
            runtime.validate_runtime_provider_models(payload, [agent])

    def test_old_or_unknown_catalog_does_not_block_or_replace_the_saved_model(self):
        agent = {"id": "worker", "providerKind": "openrouter"}
        config = {"model": "tencent/hy3-preview:free", "models": ["openrouter/free"], "apiKey": "test-fixture"}
        payload = {"providerConfigs": {"openRouter": config}}
        runtime.validate_runtime_provider_models(payload, [agent])
        config["modelCatalogCheckedAt"] = time.time() * 1000 - 6 * 60 * 1000
        runtime.validate_runtime_provider_models(payload, [agent])
        self.assertEqual(config["model"], "tencent/hy3-preview:free")


if __name__ == "__main__":
    unittest.main()
