# Contributing

Contributions are accepted under Apache-2.0. Contributors retain copyright;
contributing does not assign ownership to the maintainer or to Wanderlog.
Use a DCO sign-off (`git commit -s`) only when you can certify the contribution
under https://developercertificate.org/. DCO is not a copyright assignment or a
blanket grant to relicense third-party contributions under unrelated terms.
Separate commercial terms requiring additional rights must be negotiated with
the affected copyright holders. Commercial use under Apache-2.0 is permitted.

Keep `wanderlog-client` independent of MCP, command-line parsing, and credential
persistence. Preserve existing behavior and safety checks. Use synthetic test
data, never session cookies, trip keys, private itineraries, or confidential
upstream materials. Run `mise run ci` on macOS and the standalone client checks
in docs/publishing.md. Live end-to-end tests remain explicitly opt-in.

This project is not officially affiliated with Wanderlog. The code license is
not permission to use upstream services or trademarks. Report vulnerabilities
privately as described in SECURITY.md.
