#!/usr/bin/env python3
"""Validate Why artifacts and the references between them."""

import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]


def load_yaml(path):
    result = subprocess.run(["yq", "-o=json", "-I=0", ".", str(path)], capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError(f"yq failed for {path}: {result.stderr.strip()}")
    return json.loads(result.stdout)


def validate(schema, value, where="$"):
    """Validate the JSON Schema subset used by the Why schemas and return the violations."""
    errors = []
    kind = schema.get("type")
    checks = {
        "string": lambda v: isinstance(v, str),
        "array": lambda v: isinstance(v, list),
        "object": lambda v: isinstance(v, dict),
        "boolean": lambda v: isinstance(v, bool),
        "integer": lambda v: type(v) is int,
    }
    if kind and not checks[kind](value):
        return [f"{where}: expected {kind}"]
    if "const" in schema and value != schema["const"]:
        errors.append(f"{where}: must equal {schema['const']!r}")
    if "enum" in schema and value not in schema["enum"]:
        errors.append(f"{where}: {value!r} is not one of {schema['enum']}")
    if kind == "integer" and "minimum" in schema and value < schema["minimum"]:
        errors.append(f"{where}: must be at least {schema['minimum']}")
    if kind == "string":
        if len(value) < schema.get("minLength", 0):
            errors.append(f"{where}: must not be empty")
        if "pattern" in schema and not re.search(schema["pattern"], value):
            errors.append(f"{where}: {value!r} does not match {schema['pattern']}")
    if kind == "array":
        if len(value) < schema.get("minItems", 0):
            errors.append(f"{where}: needs at least {schema['minItems']} entries")
        if schema.get("uniqueItems") and len({json.dumps(v, sort_keys=True) for v in value}) != len(value):
            errors.append(f"{where}: entries must be unique")
        for index, item in enumerate(value):
            errors += validate(schema.get("items", {}), item, f"{where}[{index}]")
    if kind == "object":
        properties = schema.get("properties", {})
        errors += [f"{where}: missing {name}" for name in schema.get("required", []) if name not in value]
        if schema.get("additionalProperties") is False:
            errors += [f"{where}: unknown property {name}" for name in value if name not in properties]
        for name, item in value.items():
            if name in properties:
                errors += validate(properties[name], item, f"{where}.{name}")
    return errors


def load_artifacts(root, folder, prefix):
    """Load every `<prefix>-NNNN.yaml` file of a Why folder and validate it against the folder schema."""
    directory = root / "why" / folder
    if not directory.is_dir():
        return {}, []
    schema = json.loads((directory / "schema.json").read_text())
    artifacts, errors = {}, []
    for path in sorted(directory.glob(f"{prefix}-*.yaml")):
        artifact = load_yaml(path)
        problems = validate(schema, artifact, path.name)
        if not problems and artifact["id"] != path.stem:
            problems = [f"{path.name}: id {artifact['id']} does not match the file name"]
        errors += problems
        if not problems:
            artifacts[artifact["id"]] = artifact
    return artifacts, errors


def check_contracts(root, requirements):
    """Validate API indices and resolve their local resources and requirement links."""
    directory = root / "why/contracts"
    if not directory.is_dir():
        return []
    schema = json.loads((directory / "schema.json").read_text())
    errors, identifiers = [], {}
    for path in sorted(directory.rglob("api.yaml")):
        label = str(path.relative_to(root))
        api = load_yaml(path)
        problems = validate(schema, api, label)
        errors += problems
        if problems:
            continue
        for artifact in [api, *api["contracts"]]:
            identifier = artifact["id"]
            if identifier in identifiers:
                errors.append(f"{label}: duplicate ID {identifier} (also in {identifiers[identifier]})")
            identifiers[identifier] = label
        for contract in api["contracts"]:
            for reference in contract["requirements"]:
                if reference not in requirements:
                    errors.append(f"{contract['id']}: unknown requirement {reference}")
            for field, suffix in [("document", ".md"), ("schema", ".json")]:
                if field not in contract:
                    continue
                resource = path.parent / contract[field]
                relative = Path(contract[field])
                has_symlink = any((path.parent / Path(*relative.parts[:i])).is_symlink() for i in range(1, len(relative.parts) + 1))
                if has_symlink or not resource.is_file() or resource.suffix != suffix:
                    errors.append(f"{contract['id']}: {field} must reference a local {suffix} file: {contract[field]}")
                elif field == "schema":
                    try:
                        value = json.loads(resource.read_text())
                        if not isinstance(value, dict):
                            raise ValueError("expected a schema object")
                    except ValueError as error:
                        errors.append(f"{contract['id']}: invalid schema {contract[field]}: {error}")
    return errors


def check(root):
    root = root.resolve()
    requirements, errors = load_artifacts(root, "requirements", "REQ")
    errors += check_contracts(root, requirements)
    decisions, decision_errors = load_artifacts(root, "decisions", "DEC")
    errors += decision_errors
    for identifier, requirement in requirements.items():
        for reference in requirement.get("decisions", []):
            if ":" not in reference and reference not in decisions:
                errors.append(f"{identifier}: unknown decision {reference}")
    for identifier, decision in decisions.items():
        for reference in decision.get("supersedes", []):
            if reference not in decisions:
                errors.append(f"{identifier}: unknown superseded decision {reference}")
            elif reference == identifier:
                errors.append(f"{identifier}: supersedes itself")
    return requirements, decisions, errors


def main():
    requirements, decisions, errors = check(ROOT)
    assert not errors, "\n".join(errors)
    print(f"Why passed: {len(requirements)} requirements and {len(decisions)} decisions checked.")


if __name__ == "__main__":
    try:
        main()
    except AssertionError as error:  # A verdict about the Why.
        print(f"FAIL: {error}", file=sys.stderr)
        sys.exit(1)
    except Exception as error:  # A crash is a technical failure, never a FAIL verdict.
        print(f"ERROR: {type(error).__name__}: {error}", file=sys.stderr)
        sys.exit(2)
