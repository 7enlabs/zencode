#!/usr/bin/env python3
"""
Validate the APP-2 M1 companion-ws v1 schemas against the stream-json v1 fixtures.

Schemas live next to this script in docs/schemas/companion-ws-v1/; the fixture
copies in ./fixtures/stream-json are byte-identical to docs/fixtures/stream-json/
(source of truth - refresh the copies when the source fixtures change).

Checks:
  * every fixture record is valid against envelope + its per-type(/subtype) schema
  * every companion-event fixture record in ./fixtures/companion is valid
    against envelope + its per-event schema
  * generated negative cases are rejected
  * one compatibility case (unknown extra field) stays valid (v1 ignore-unknown rule)

Uses the `jsonschema` library (Draft 2020-12) when importable. If it is not
installed, falls back to a small built-in subset validator (type / required /
const / enum / properties / items / minLength) and says so in the output.
Nothing is installed by this script.
"""
import copy
import glob
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
SCHEMA_DIR = HERE
FIXTURE_DIR = os.path.join(HERE, "fixtures", "stream-json")
COMPANION_FIXTURE_DIR = os.path.join(HERE, "fixtures", "companion")

try:
    from jsonschema import Draft202012Validator

    MODE = "jsonschema library (Draft 2020-12)"

    def make_validator(schema):
        Draft202012Validator.check_schema(schema)
        return Draft202012Validator(schema).is_valid

except ImportError:
    MODE = "builtin subset validator (jsonschema library not available)"

    def _type_ok(value, t):
        return {
            "object": lambda v: isinstance(v, dict),
            "array": lambda v: isinstance(v, list),
            "string": lambda v: isinstance(v, str),
            "number": lambda v: isinstance(v, (int, float)) and not isinstance(v, bool),
            "boolean": lambda v: isinstance(v, bool),
            "null": lambda v: v is None,
        }[t](value)

    def _check(instance, schema):
        if "const" in schema and instance != schema["const"]:
            return False
        if "enum" in schema and instance not in schema["enum"]:
            return False
        t = schema.get("type")
        if t is not None:
            types = t if isinstance(t, list) else [t]
            if not any(_type_ok(instance, x) for x in types):
                return False
        if isinstance(instance, str) and "minLength" in schema:
            if len(instance) < schema["minLength"]:
                return False
        if isinstance(instance, dict):
            for req in schema.get("required", []):
                if req not in instance:
                    return False
            for key, sub in schema.get("properties", {}).items():
                if key in instance and not _check(instance[key], sub):
                    return False
        if isinstance(instance, list) and "items" in schema:
            for item in instance:
                if not _check(item, schema["items"]):
                    return False
        return True

    def make_validator(schema):
        return lambda instance: _check(instance, schema)


def load_schemas():
    schemas = {}
    for path in sorted(glob.glob(os.path.join(SCHEMA_DIR, "*.schema.json"))):
        name = os.path.basename(path).replace(".schema.json", "")
        with open(path, encoding="utf-8") as fh:
            schemas[name] = make_validator(json.load(fh))
    return schemas


def applicable(record, schemas):
    """Return the list of schema names a record must satisfy."""
    names = ["envelope"]
    t = record.get("type")
    if t == "system":
        names.append("system")
        sub = record.get("subtype")
        if f"system-{sub}" in schemas:
            names.append(f"system-{sub}")
    elif t in schemas:
        names.append(t)
    return names


def is_valid(record, schemas):
    return all(schemas[name](record) for name in applicable(record, schemas))


def main():
    print(f"validation mode: {MODE}")
    schemas = load_schemas()
    print(f"schemas loaded: {len(schemas)}")

    valid_total = valid_pass = 0
    per_file = []
    records = {}
    fixture_paths = sorted(glob.glob(os.path.join(FIXTURE_DIR, "*.ndjson")))
    fixture_paths += sorted(glob.glob(os.path.join(COMPANION_FIXTURE_DIR, "*.ndjson")))
    for path in fixture_paths:
        fname = os.path.basename(path)
        n = ok = 0
        with open(path, encoding="utf-8") as fh:
            for line in fh:
                line = line.strip()
                if not line:
                    continue
                rec = json.loads(line)
                records.setdefault(fname, []).append(rec)
                n += 1
                if is_valid(rec, schemas):
                    ok += 1
                else:
                    print(f"  INVALID (should be valid): {fname}: {rec.get('type')}/{rec.get('subtype')}")
        per_file.append((fname, ok, n))
        valid_total += n
        valid_pass += ok
    for fname, ok, n in per_file:
        print(f"  fixture {fname}: {ok}/{n} records valid")
    print(f"VALID CASES: {valid_pass}/{valid_total} passed")

    # --- negative + compatibility cases, generated from real fixture records ---
    plain = records["plain_text.ndjson"]
    init_rec = next(r for r in plain if r.get("subtype") == "init")
    asst_rec = next(r for r in plain if r.get("type") == "assistant")
    result_rec = next(r for r in plain if r.get("type") == "result")
    events = records["events.ndjson"]
    prompt_rec = next(r for r in events if r.get("type") == "companion_prompt")
    status_rec = next(r for r in events if r.get("type") == "host_status")

    def without(rec, field):
        c = copy.deepcopy(rec)
        c.pop(field, None)
        return c

    def changed(rec, field, value):
        c = copy.deepcopy(rec)
        c[field] = value
        return c

    negatives = [
        ("missing v", without(init_rec, "v"), "envelope"),
        ("v changed to 2", changed(init_rec, "v", 2), "envelope"),
        ("unknown type", changed(asst_rec, "type", "unknown_type"), "envelope"),
        ("missing session_id", without(asst_rec, "session_id"), "envelope"),
        ("system record missing subtype", without(init_rec, "subtype"), "system"),
        ("result record missing usage", without(result_rec, "usage"), "result"),
        ("companion_prompt missing text", without(prompt_rec, "text"), "companion_prompt"),
        ("companion_prompt missing uuid", without(prompt_rec, "uuid"), "companion_prompt"),
        ("host_status missing state", without(status_rec, "state"), "host_status"),
        ("host_status state outside enum", changed(status_rec, "state", "busy"), "host_status"),
    ]
    neg_ok = 0
    for label, rec, schema_name in negatives:
        rejected = not schemas[schema_name](rec)
        print(f"  negative '{label}' vs {schema_name}: {'rejected (ok)' if rejected else 'ACCEPTED (BAD)'}")
        neg_ok += rejected
    print(f"NEGATIVE CASES: {neg_ok}/{len(negatives)} rejected as expected")

    compat_rec = changed(asst_rec, "x_unknown_future_field", {"anything": [1, 2, 3]})
    compat_ok = schemas["envelope"](compat_rec) and schemas["assistant"](compat_rec)
    print(f"COMPAT CASE (unknown extra field stays valid): {'passed' if compat_ok else 'FAILED'}")

    all_ok = valid_pass == valid_total and neg_ok == len(negatives) and compat_ok
    print("OVERALL:", "PASS" if all_ok else "FAIL")
    return 0 if all_ok else 1


if __name__ == "__main__":
    sys.exit(main())
