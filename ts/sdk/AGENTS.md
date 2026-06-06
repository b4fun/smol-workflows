# SDK maintenance notes

This directory contains the `@smol-workflow/sdk` TypeScript package. The package provides workflow authoring types and ambient declarations for runtime-injected workflow APIs.

When changing public SDK APIs:

- Update `src/` types first; generated `dist/` files are produced by `npm run build`.
- Update `README.md` when workflow author usage changes.
- Update `changelogs.md` for every user-visible API/type change, including added fields, removed fields, renamed types, changed global declarations, and behavior-affecting documentation updates.
- Bump `package.json` version when preparing a publishable release.
- Run `npm run typecheck` and `npm run build` from this directory before release.

Publishing is handled by GitHub Actions from the repository root workflow `.github/workflows/npm-sdk-publish.yml`. To publish, ensure the repository has an `NPM_TOKEN` secret with publish access, then push a tag named `ts/sdk/v<package-version>` or run the workflow manually.
