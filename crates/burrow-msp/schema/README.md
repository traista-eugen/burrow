# MSP wire schema

`msp.schema.json` and `manifest.json` are Muse Code's own export of its MSP wire schema, committed
unchanged. The Rust types in `src/types/` are written by hand against it, and the tests check them
against it (decision D-022 in `docs/decisions.md`).

| | |
|---|---|
| Muse version | Muse Code 1.4.14 (`1.4.14-R7309.1`) |
| Surface | stable (not `--experimental`) |
| Envelope schema version | 1 |
| Fingerprint | `sha256:c94c3d1f2f51e6424074b9ec43eac3eedcf532d3dcd77351ec1383a87d3049c6` |
| Exported | 2026-11-09 |

`initialize` returns the same fingerprint as `schema.fingerprint`. `burrow-msp` compares the two
and logs a warning when they differ (`MspClient::schema_matches`).

## Regenerating

```powershell
muse --version
muse schema generate-json-schema --out crates/burrow-msp/schema
```

Then update the table above, run `cargo test -p burrow-msp`, and fix whatever the tests report:
the type checks in `src/schema_check.rs` and the live frame validation in `tests/echo.rs`.

The descriptions in the export contain mojibake (`â€”` for `—`). That's how Muse writes them; the
file is kept byte for byte so a fresh export diffs cleanly.
