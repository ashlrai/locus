# Adapter template

Copy this folder when adding a new built-in provider adapter to Locus.

## Steps

1. **Copy skeleton** into the core crate:

   ```bash
   cp examples/adapters/_template/skeleton.rs \
      crates/locus-core/src/adapters/myprovider.rs
   ```

2. **Rename** the unit struct / provider id (`myprovider` → your id).

3. **Register** in `crates/locus-core/src/adapters/mod.rs`:

   ```rust
   mod myprovider;
   pub use myprovider::MyProviderAdapter;

   // in adapter_for():
   "myprovider" => Some(Box::new(MyProviderAdapter)),
   ```

4. **Freeze every account selector** the model might smuggle (`freeze_string_arg`,
   `freeze_bool_arg`, or an allowlist check).

5. **Expose at least** `myprovider.scope` (identity dump, no secrets).

6. **Add tests** (freeze deny + happy path) in the module or `adapters/mod.rs`.

7. **Catalog** the provider in `adapters/manifest.toml`.

8. **Docs**: row in `docs/adapters.md` / `docs/adapter-sdk.md` if user-facing.

Full guide: [docs/adapter-sdk.md](../../../docs/adapter-sdk.md).

## Rules

| Do | Don't |
|----|--------|
| Fail closed on scope mismatch | Soft-allow wrong `project_ref` / team |
| CredentialRefs only (`phm:`, `env:`) | Put raw tokens in binding TOML or tool results |
| Mark mutating stubs `destructive: true` | Skip `require_approval` coverage |
| Keep provider knowledge in the adapter | Scatter freeze logic across CLI/MCP |

## Verify

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test -p locus-core
cargo clippy -p locus-core --all-targets -- -D warnings
```
