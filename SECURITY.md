# Security

Do not put passwords, `connect.sid` cookies, trip keys, private itineraries or
bookings in public issues, pull requests, logs or test fixtures. Trip keys are
bearer capabilities. Revoke upstream sessions after a leak.

Report vulnerabilities with GitHub private vulnerability reporting if enabled,
or use an existing private channel to the repository owner before disclosing
details. No guaranteed response time or perpetual maintenance is promised.

The reusable client accepts caller-provided credentials without persisting them.
The local application retains macOS Keychain integration. Low-level client writes
do not inherit MCP-level read-only mode, redaction, duplicate-write detection or
confirmation behavior: check permissions, revisions and uncertain outcomes.
Never blindly replay writes after an uncertain result.

This is an unofficial private-API integration and the upstream service can
change without notice. Local execution does not prevent an AI client from
receiving the itinerary content returned by a tool. See PRIVACY.md.
