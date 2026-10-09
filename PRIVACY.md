# Privacy and data handling

This document describes the local Wanderlog MCP project, not Wanderlog's service
or the AI application launching the server.

The application runs locally. It obtains credentials from WANDERLOG_COOKIE or
the existing macOS Keychain entry. The reusable client accepts credentials from
its caller and does not store them. A session cookie grants broad account
access and must be kept secret.

When invoked, tools contact Wanderlog for the requested operations. Returned
itinerary and place information is passed to the AI client, whose own processing
and retention policies apply. Local execution does not mean all returned data
stays on the device. No project-operated remote MCP or telemetry service is part
of this implementation. Package installation and dependency updates may contact
GitHub, package registries and other providers with separate privacy policies.

`wanderlog-mcp auth clear` removes the Keychain entry. Remove injected environment
cookies separately and revoke upstream sessions where appropriate. Never send
credentials or real bookings in bug reports.
