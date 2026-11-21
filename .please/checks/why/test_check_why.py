"""The Why check judges artifacts and references and reports crashes as exit 2."""

import importlib.util
import json
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().with_name("check.py")
REQUIREMENT_SCHEMA = SCRIPT.parents[3] / "why/requirements/schema.json"
API_SCHEMA = SCRIPT.parents[3] / "why/contracts/schema.json"
DECISION_SCHEMA = SCRIPT.parents[3] / "why/decisions/schema.json"
spec = importlib.util.spec_from_file_location("why_check", SCRIPT)
check = importlib.util.module_from_spec(spec)
spec.loader.exec_module(check)


class SchemaSubsetTests(unittest.TestCase):
    def test_violations_are_reported_with_their_location(self):
        schema = json.loads(REQUIREMENT_SCHEMA.read_text())
        broken = {"id": "REQ-1", "name": "", "description": "Text.", "rationale": "Reason.", "extra": True}
        errors = check.validate(schema, broken, "x")
        self.assertTrue(any("x.id" in error for error in errors), errors)
        self.assertTrue(any("x.name" in error for error in errors), errors)
        self.assertTrue(any("unknown property extra" in error for error in errors), errors)

@unittest.skipUnless(shutil.which("yq"), "yq is required to read YAML")
class RepositoryTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.root = Path(self.directory.name)

    def tearDown(self):
        self.directory.cleanup()

    def errors(self):
        return check.check(self.root)[2]

    def test_a_repository_without_artifacts_passes(self):
        self.assertEqual(self.errors(), [])

    def write_requirement(self, **changes):
        requirements = self.root / "why/requirements"
        requirements.mkdir(parents=True, exist_ok=True)
        shutil.copy(REQUIREMENT_SCHEMA, requirements / "schema.json")
        value = {
            "id": "REQ-0001",
            "name": "Name",
            "description": "The ledger must do one thing.",
            "rationale": "A reason.",
        } | changes
        (requirements / f"{value['id']}.yaml").write_text(json.dumps(value))

    def write_api(self, folder="adapter", **changes):
        self.write_requirement()
        directory = self.root / "why/contracts"
        directory.mkdir(parents=True, exist_ok=True)
        shutil.copy(API_SCHEMA, directory / "schema.json")
        directory = directory / folder
        directory.mkdir(exist_ok=True)
        (directory / "request.md").write_text("# Request\n")
        (directory / "request.schema.json").write_text('{"type":"object"}')
        value = {
            "id": "API-0001",
            "name": "Adapter",
            "version": 0,
            "contracts": [
                {
                    "id": "CON-0001",
                    "name": "Request",
                    "document": "request.md",
                    "schema": "request.schema.json",
                    "requirements": ["REQ-0001"],
                }
            ],
        } | changes
        (directory / "api.yaml").write_text(json.dumps(value))
        return directory

    def test_api_indices_resolve_contracts_and_requirements(self):
        self.write_api()
        self.assertEqual(self.errors(), [])

    def test_api_version_must_be_a_nonnegative_integer(self):
        for version in [-1, True, "0", 0.5]:
            self.write_api(version=version)
            self.assertTrue(any("version" in error for error in self.errors()), version)

    def test_api_and_contract_ids_are_globally_unique(self):
        self.write_api()
        self.write_api("other")
        errors = self.errors()
        for identifier in ["API-0001", "CON-0001"]:
            self.assertTrue(any(f"duplicate ID {identifier}" in error for error in errors), errors)

    def test_contract_requirement_references_must_resolve(self):
        self.write_api()
        (self.root / "why/requirements/REQ-0001.yaml").unlink()
        self.assertTrue(any("unknown requirement REQ-0001" in error for error in self.errors()))

    def test_contract_resources_must_exist_and_stay_local(self):
        directory = self.write_api()
        index = json.loads((directory / "api.yaml").read_text())
        for resource in ["missing.md", "../request.md", "/request.md", "request.schema.json"]:
            index["contracts"][0]["document"] = resource
            (directory / "api.yaml").write_text(json.dumps(index))
            self.assertTrue(self.errors(), resource)
        index["contracts"][0]["document"] = "request.md"
        (directory / "api.yaml").write_text(json.dumps(index))
        (directory / "request.md").unlink()
        (directory / "request.md").symlink_to(directory / "request.schema.json")
        self.assertTrue(any("local .md file" in error for error in self.errors()))

    def test_contract_schema_must_be_a_json_object(self):
        directory = self.write_api()
        for content in ["{invalid", "[]"]:
            (directory / "request.schema.json").write_text(content)
            self.assertTrue(any("invalid schema" in error for error in self.errors()))

    def test_api_indices_reject_unknown_metadata(self):
        self.write_api(extra="unrecognized")
        self.assertTrue(any("unknown property extra" in error for error in self.errors()))

    def test_a_valid_requirement_passes(self):
        self.write_requirement()
        self.assertEqual(self.errors(), [])

    def test_a_requirement_cannot_cite_a_missing_decision(self):
        self.write_requirement(decisions=["DEC-0099"])
        self.assertTrue(any("unknown decision DEC-0099" in error for error in self.errors()))

    def write_decision(self, **changes):
        decisions = self.root / "why/decisions"
        decisions.mkdir(parents=True, exist_ok=True)
        shutil.copy(DECISION_SCHEMA, decisions / "schema.json")
        value = {"id": "DEC-0001", "name": "Name", "context": "Why.", "decision": "What.", "rationale": "Because."} | changes
        (decisions / f"{value['id']}.yaml").write_text(json.dumps(value))

    def test_a_valid_decision_passes(self):
        self.write_decision()
        self.assertEqual(self.errors(), [])

    def test_a_decision_cannot_reference_mutable_artifacts(self):
        self.write_decision(requirements=["REQ-0001"])
        self.assertTrue(any("unknown property requirements" in error for error in self.errors()))

    def test_a_requirement_violating_its_schema_fails(self):
        self.write_requirement(rationale="")
        self.assertTrue(any("rationale" in error for error in self.errors()))

    def test_an_artifact_must_be_named_after_its_id(self):
        self.write_requirement()
        requirements = self.root / "why/requirements"
        (requirements / "REQ-0001.yaml").rename(requirements / "REQ-0002.yaml")
        self.assertTrue(any("does not match the file name" in error for error in self.errors()))

    def test_a_decision_violating_its_schema_fails(self):
        self.write_decision(context="")
        self.assertTrue(any("context" in error for error in self.errors()))

    def test_a_decision_can_supersede_an_existing_decision(self):
        self.write_decision()
        self.write_decision(id="DEC-0002", supersedes=["DEC-0001"])
        self.assertEqual(self.errors(), [])

    def test_a_decision_cannot_supersede_a_missing_or_its_own_decision(self):
        self.write_decision(supersedes=["DEC-0099"])
        self.assertTrue(any("unknown superseded decision DEC-0099" in error for error in self.errors()))
        self.write_decision(supersedes=["DEC-0001"])
        self.assertTrue(any("supersedes itself" in error for error in self.errors()))

    def test_a_requirement_can_cite_an_existing_decision(self):
        self.write_decision()
        self.write_requirement(decisions=["DEC-0001"])
        self.assertEqual(self.errors(), [])

    def test_a_requirement_can_cite_a_decision_of_another_repository_without_resolving_it(self):
        self.write_requirement(decisions=["why:DEC-0002"])
        self.assertEqual(self.errors(), [])

    def test_a_decision_reference_accepts_only_a_plain_repository_qualifier(self):
        for reference in ["why:", ":DEC-0002", "Why:DEC-0002", "a:b:DEC-0002", "why:DEC-2"]:
            self.write_requirement(decisions=[reference])
            self.assertTrue(any("decisions[0]" in error for error in self.errors()), reference)

    def test_a_decision_cannot_supersede_a_decision_of_another_repository(self):
        self.write_decision(supersedes=["why:DEC-0002"])
        self.assertTrue(any("supersedes[0]" in error for error in self.errors()))


class ExitTests(unittest.TestCase):
    def run_check(self, root):
        copy = root / ".please/checks/why/check.py"
        copy.parent.mkdir(parents=True)
        copy.write_text(SCRIPT.read_text())
        return subprocess.run([sys.executable, str(copy)], capture_output=True, text=True)

    def test_a_repository_without_artifacts_passes(self):
        with tempfile.TemporaryDirectory() as directory:
            self.assertEqual(self.run_check(Path(directory)).returncode, 0)

    def test_a_violation_fails_with_exit_1(self):
        with tempfile.TemporaryDirectory() as directory:
            requirements = Path(directory) / "why/requirements"
            requirements.mkdir(parents=True)
            shutil.copy(REQUIREMENT_SCHEMA, requirements / "schema.json")
            (requirements / "REQ-0001.yaml").write_text("{}")
            self.assertEqual(self.run_check(Path(directory)).returncode, 1)

    @unittest.skipUnless(shutil.which("yq"), "yq is required to read YAML")
    def test_an_unreadable_artifact_is_an_error(self):
        with tempfile.TemporaryDirectory() as directory:
            requirements = Path(directory) / "why/requirements"
            requirements.mkdir(parents=True)
            shutil.copy(REQUIREMENT_SCHEMA, requirements / "schema.json")
            (requirements / "REQ-0001.yaml").write_text("a: [unclosed")
            result = self.run_check(Path(directory))
            self.assertEqual(result.returncode, 2, result.stderr)


if __name__ == "__main__":
    unittest.main()
