# Config schema-conformance corpus

Shared, language-neutral fixtures that pin **identical JSON-Schema validation
verdicts** between the Rust config store (`penguin-daemon::config`, jsonschema
0.47) and the frozen Go config store (`go-client/internal/daemon`,
santhosh-tekuri/jsonschema v6). This is a permanent CI fixture — the two engines
must agree on every case.

## Format

One JSON file per case:

```json
{
  "description": "human summary",
  "valid": true,
  "schema": { "$schema": "https://json-schema.org/draft/2020-12/schema", ... },
  "instanceYaml": "key: value\n"
}
```

- `valid` — the expected verdict: `true` means the instance passes the schema,
  `false` means it is rejected.
- `schema` — a JSON Schema (draft 2020-12, pinned via `$schema` so both engines
  select the same dialect).
- `instanceYaml` — the module config as YAML text. Each harness writes it to a
  temp `modules.d/<name>.yaml` and runs it through its real config store
  (`ConfigStore::module` / `cs.Module`), so the YAML→JSON bridge is exercised too.

## Scope

Cases stay within the draft 2020-12 keywords both engines implement identically
(`type`, `required`, `properties`, `additionalProperties`, `enum`, `minimum`,
`maximum`, `minLength`, `pattern`, `items`, `minItems`). Dialect-divergent
features (`$ref`, `format` assertions, `if`/`then`, `unevaluatedProperties`) are
deliberately excluded — that is the whole point of the parity gate.

## Harnesses

- Rust: `crates/penguin-daemon/tests/config_corpus.rs`
- Go (oracle): `go-client/internal/daemon/corpus_conformance_test.go`
