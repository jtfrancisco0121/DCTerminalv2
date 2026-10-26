# Notices

The files under `plugin/bmad/` are third-party work, used under the MIT License.

- `skills/` is a copy of selected skills from BMAD-METHOD
  (https://github.com/bmad-code-org/BMAD-METHOD), pinned at the commit in `VERSION`,
  with the Python scripts and tests removed and the text patched by
  `scripts/patch-bmad.mjs`. Copyright (c) 2025 BMad Code, LLC. The licence is in `LICENSE`.
- `ade-runtime.mjs` is the Node port of those Python scripts from ADE
  (https://github.com/alvin-reyes/better-agentic-ide, commit 7e8c606), unchanged apart
  from a header comment. MIT License, Copyright (c) 2025-2026 Alvin Reyes.
- `scripts/patch-bmad.mjs` is a port to plain JS of ADE's `scripts/patchBmadSkills.ts`.
  MIT License, Copyright (c) 2025-2026 Alvin Reyes.
- `runtime.mjs` is DCTerminalv2's own wrapper around `ade-runtime.mjs`.

ADE's MIT licence text:

> Copyright (c) 2025-2026 Alvin Reyes
>
> Permission is hereby granted, free of charge, to any person obtaining a copy of this
> software and associated documentation files (the "Software"), to deal in the Software
> without restriction, including without limitation the rights to use, copy, modify,
> merge, publish, distribute, sublicense, and/or sell copies of the Software, and to
> permit persons to whom the Software is furnished to do so, subject to the following
> conditions:
>
> The above copyright notice and this permission notice shall be included in all copies
> or substantial portions of the Software.
>
> THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED,
> INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
> PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT
> HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF
> CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE
> OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

"BMAD" and "BMAD-METHOD" are names of the upstream project. DCTerminalv2 uses them only to
say that it is compatible with the BMAD-METHOD skills (the UI says "BMAD-METHOD skills");
it is not affiliated with or endorsed by the BMAD-METHOD project.
