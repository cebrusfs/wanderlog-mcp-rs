# Pinned license texts

`rmcp` and `rmcp-macros` 3.5.1 declare Apache-2.0, but their crate archives omit the license
text ([upstream issue](https://github.com/modelcontextprotocol/rust-sdk/issues/1335)).
`rmcp-3.5.1.txt` is the unmodified upstream `LICENSE` at commit
`79437f291b2c44053d00dcd5db969fd0cca7c887`:
<https://raw.githubusercontent.com/modelcontextprotocol/rust-sdk/79437f291b2c44053d00dcd5db969fd0cca7c887/LICENSE>
(SHA-256 `0382b0057770ca05e9c350a50aa3b1c1fea84da0bc81d723bf00b9aa841be58a`). It keeps the
upstream MIT-to-Apache transition and documentation-licensing text.

[`third-party-notices.mjs`](../third-party-notices.mjs) applies an override only
to the exact crate versions it names, and fails when a shipped crate has no license text or an
override no longer matches its pinned hash. When upgrading these crates, drop the override once
the issue is fixed and the new archives contain `LICENSE`.
