# Lingo.dev-Compatible Localization

DX i18n is local-first. Local catalogs, source JSON, Markdown, and lock/hash checks work without Lingo.dev credentials.

## Modes

### Local-only

Use `LocalCatalog`, `LocalFirstLocalizer`, `JsonLocalizationDocument`, `ProtectedText`, and `I18nLock` when the project should stay offline. Missing local translations fall back to source text, and no cloud auth is required.

### Lingo.dev-compatible

Use `LingoApiProvider` only when a workflow explicitly opts into a Lingo.dev-compatible engine. The provider targets the current synchronous API shape:

```text
POST https://api.lingo.dev/process/localize
```

The request body includes `engineId` when configured, `sourceLocale`, `targetLocale`, and `data`. API keys are sent as `X-API-Key` headers by the HTTP client and are never embedded in request JSON or fixtures.

## Environment

The Rust adapter checks DX-specific variables first, then Lingo.dev-compatible names:

```text
DX_I18N_LINGO_API_KEY
LINGO_API_KEY
LINGODOTDEV_API_KEY
DX_I18N_LINGO_ENGINE_ID
LINGO_ENGINE_ID
LINGO_API_URL
LINGO_API_BASE_URL
```

`LINGO_API_KEY` and `LINGO_API_URL` are the current Lingo.dev-documented engine environment variables. `LINGODOTDEV_API_KEY` remains supported as a legacy compatibility alias, and `DX_I18N_LINGO_API_KEY` takes precedence for DX-specific workflows. Blank higher-priority environment aliases are ignored so lower-priority configured aliases can still be used; an explicit blank `--api-key` override is rejected instead of falling through to ambient credentials. `LINGO_API_BASE_URL` remains supported as a DX compatibility alias. Both API URL variables default to `https://api.lingo.dev` and exist mainly for tests or trusted proxies. Custom API URLs must use HTTPS unless they target localhost or loopback, and they must not include credentials, query strings, or fragments.

`LingoApiProvider::new` builds a bounded HTTP client with a 10 second connect timeout, 30 second request timeout, bounded successful response bodies, bounded and redacted error response bodies, and redirect following disabled so a Lingo API key is not forwarded to a redirected endpoint. Custom API URLs are supported for the official `LINGO_API_URL` use case, local mocks, and trusted proxies; DX does not prove the safety of arbitrary third-party endpoints, so configure only endpoints you trust to receive the Lingo API key.

## Config And Lockfile

The config model accepts Lingo.dev-style `i18n.json` fields: `$schema`, `version`, `locale.source`, `locale.targets`, `buckets`, optional `engineId`, and optional `provider`.

Configured JSON and Markdown buckets honor `include`, `exclude`, delimiter-based `[locale]` path expansion, and key controls including `lockedKeys`, `preservedKeys`, `ignoredKeys`, and `injectLocale` in local workspace operations. Local-only workspace commands validate locale and bucket structure without rejecting incomplete optional remote provider metadata; remote provider validation remains available through full config validation. Lingo-supported bucket types that the Rust workspace does not yet implement are reported by `status` as `unsupported_bucket=<type>` and block unfiltered `lockfile`/`run` execution so partial Rust support cannot create a false-green update. Filtered `run --bucket json` skips unselected unsupported buckets. Unknown bucket types fail config validation.

The lock model stores and parses `version: 1` plus SHA-256 source and key fingerprints. It can classify source units as current, renamed, changed, or new. Current DX support is intentionally parse/write compatibility; it does not claim to replicate every Lingo.dev CLI migration behavior.

The parser accepts both 64-character SHA-256-style hashes from the public docs and 32-character hash values seen in the current official CLI repository lockfile format. DX-generated lockfiles use SHA-256-style hashes.

## Rust Command Boundary

The `dx-i18n` binary is a small Rust-side command boundary over `LocalizationWorkspace`:

```text
dx-i18n [--root <path>] show config
dx-i18n [--root <path>] status
dx-i18n [--root <path>] status --locale es --locale fr
dx-i18n [--root <path>] status --bucket json --bucket markdown
dx-i18n [--root <path>] status --file common.json --file docs/
dx-i18n [--root <path>] status --force
dx-i18n [--root <path>] status --verbose
dx-i18n [--root <path>] lockfile
dx-i18n [--root <path>] lockfile --check
dx-i18n [--root <path>] lockfile --force
dx-i18n [--root <path>] run
dx-i18n [--root <path>] run --target-locale <locale> --target-locale <locale>
dx-i18n [--root <path>] run --bucket json --bucket markdown
dx-i18n [--root <path>] run --file common.json --file docs/
dx-i18n [--root <path>] run --key auth.login --key frontmatter.title
dx-i18n [--root <path>] run --target-locale es --bucket json --file common.json --key auth.login
dx-i18n [--root <path>] run --target <locale> --frozen
dx-i18n [--root <path>] run --target <locale> --force
dx-i18n [--root <path>] run --target <locale> --lingo
dx-i18n [--root <path>] run --target <locale> --mode lingo
dx-i18n [--root <path>] run --target <locale> --lingo --api-key <api-key>
dx-i18n [--root <path>] run --target <locale> --lingo --force
dx-i18n [--root <path>] run --target <locale> --provider openai
```

`status`, `lockfile`, and `lockfile --check` read configured JSON and Markdown source buckets without cloud auth. `status` is read-only and supports repeatable `--locale`, `--bucket`, and `--file` filters over the status report; it intentionally does not accept `--key`, because the current official Lingo.dev status command does not document a key filter. Status locale filters are validated against configured target locales and also scope target-path file matching. `status` reports lockfile-pending source units separately from rendered target-file drift, so a current `i18n.lock` can still identify target files that would fail `run --frozen`. `status --verbose` prints pending source-file/key detail and drifted target file paths; it is compatible with `--locale`, `--bucket`, `--file`, and `--force`; it remains a local report and does not enable remote/provider status. `status --force` bypasses lockfile change detection for retranslation estimates while still respecting locked, preserved, ignored, and injected key controls; it does not rewrite `i18n.lock` or target files. Plain `run` is local by default: it renders JSON and Markdown targets from existing localizations and source fallback for all configured target locales, then writes a fresh `i18n.lock` after a successful non-frozen unfiltered run. `--target-locale` is repeatable, and `--target` is accepted as a compatibility alias. `run` supports repeatable `--bucket`, `--file`, and `--key` filters. Bucket filters match configured bucket types exactly, file filters use substring matching over normalized include/source/target paths, and key filters normalize dots to slash-separated key prefixes such as `auth.login` -> `auth/login`. Filtered non-frozen runs fail before output or lockfile writes when file filters select no source files or key filters select no localization units. Successful filtered runs merge selected source fingerprints into the existing lockfile while removing stale fingerprints for the same selected keys and preserving current unselected same-key entries. Requested target locales and filters are validated before the first output write. `run --frozen` is a no-write verification gate over the selected scope: it checks selected source lock state, compares selected rendered target outputs, and refuses stale selected source content or target drift before writing. Local `run --force` is accepted and stays auth-free; it rewrites from local source/target data without creating a provider. `--force` and `--frozen` are mutually exclusive, because one requests rewriting while the other promises no writes. `lockfile --check` compares parsed lock state instead of YAML bytes; plain `lockfile` creates a missing lockfile or reports an existing current lockfile, while `lockfile --force` explicitly rewrites. `run --target <locale> --lingo` and `--mode lingo` explicitly opt into the Lingo.dev-compatible HTTP adapter and require `DX_I18N_LINGO_API_KEY`, `LINGO_API_KEY`, `LINGODOTDEV_API_KEY`, or the explicit non-empty `--api-key` override. In Lingo mode, the default run sends only lockfile-pending provider-eligible units, preserves safe existing target strings for current units, leaves target files and `i18n.lock` untouched when no provider translations pass DX safety checks, and skips provider requests when nothing is pending. Filtered Lingo runs merge lock fingerprints only for provider-accepted keys and prune stale entries for those selected keys. `run --lingo --force` bypasses lockfile delta selection and re-sends all selected provider-eligible units while still writing only provider-accepted changes and respecting locked, preserved, ignored, and injected key controls. If `i18n.json` has `engineId`, the CLI passes it through unless an engine ID is already set by env.

The CLI remote path currently targets the Lingo.dev Engine-compatible API. Raw provider configs are parsed and validated for compatibility. The command parser also recognizes the current Lingo raw provider IDs (`openai`, `anthropic`, `google`, `mistral`, `openrouter`, and `ollama`) as an explicit Rust-side boundary, but direct raw-provider execution is not implemented in the Rust CLI. Those commands fail before writing output files.

Because `ollama` is treated as an auth-free raw-provider config, its `provider.baseUrl` must target localhost or loopback and must not include credentials, query strings, or fragments.

The Rust CLI intentionally still rejects official Lingo options that are not implemented here, including status `--api-key` and run `--source-locale`, `--watch`, `--concurrency`, `--debounce`, `--debug`, and `--sound`.

## Structure Safety

`ProtectedText` replaces sensitive spans with collision-avoiding sentinels before translation and restores them after translation. It protects placeholders, double-brace interpolation tokens, ICU-style variables including plural offsets and exact-match selectors, inline code, and fenced Markdown code blocks. Restore rejects missing or duplicated sentinels. JSON localization keeps the original structure and only replaces string values at matching escaped key paths, including arrays and empty source objects discovered from JSON documents. Markdown rendering copies source frontmatter metadata and code fences while reusing existing target text for translatable frontmatter titles, headings, list items, paragraphs, and quotes. Nested blockquote marker prefixes are preserved when applying translations. Existing target JSON and Markdown strings are reused only when protected placeholder, code, ICU branch marker, Markdown link/emphasis/table, table-cell link/emphasis/HTML placement, HTML tag, line, block-marker, and Markdown unit-order structure is compatible with the source. Repeated same-kind Markdown units without stable structural anchors are treated as ambiguous, including partial target files, so local rendering falls back to source text instead of guessing by ordinal position. Local rendering falls back to source text for unsafe target units; Lingo delta rendering sends current-but-missing or current-but-unsafe provider-eligible units to the provider instead of silently carrying source text forward. Key-filtered runs leave unselected existing target strings untouched when present, and fall back to source text for missing unselected strings.

## SDK Status

As verified against official Lingo.dev docs and GitHub on 2026-05-25, Lingo.dev documents JS/Python/PHP/Ruby SDKs and has an official `lingodotdev/sdk-rust` repository, but that Rust repository is empty. DX therefore uses a minimal Rust HTTP adapter boundary instead of depending on a non-existent Rust SDK.

Network tests should stay mocked or explicitly gated by environment variables.
