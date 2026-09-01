# Contributing

Thanks for helping improve Codex Quota.

## Before opening an issue

- Search existing issues first.
- Include the Codex Quota version, Windows version, and Codex installation type.
- Remove account names, local paths, tokens, prompts, and conversation content from logs.
- Describe the expected quota values and the values displayed by the app.

## Pull requests

1. Keep changes focused.
2. Run `npm run build`.
3. Run `cargo test` and `cargo clippy --all-targets -- -D warnings` in `src-tauri`.
4. Preserve English and Simplified Chinese strings when changing visible UI.
5. Respect reduced-motion and reduced-transparency preferences.

