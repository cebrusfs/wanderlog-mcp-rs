# Privacy

wanderlog-mcp runs locally as a stdio MCP server and CLI. This project operates no hosted
service and collects no telemetry or analytics.

**What leaves your machine.** When a tool runs, the server sends your Wanderlog session cookie
and the needed requests to Wanderlog, whose [privacy policy](https://wanderlog.com/privacy)
applies there. Tool results, including itinerary data and notes, go to your MCP client, which may
send them to its AI provider under that provider's policies.

**Credentials.** `auth login` exchanges your email and password for a session cookie and stores
only the cookie, in the macOS Keychain; the email and password are not kept. A cookie can instead
be supplied through the `WANDERLOG_COOKIE` environment variable (or the optional field in the
Claude Desktop extension). Prefer the Keychain: other processes running as your user can read a
process's environment. `auth clear` deletes the stored cookie but does not end the session on
Wanderlog's side.

**Local data.** Trip lists, place details, rate-limit state and retry guards are kept in memory
only and are discarded when the server exits; a new session also starts with empty trip and place
caches. The server writes no itinerary files. Trip keys and the cookie are removed from tool output and error messages; avoid enabling
request logging or pasting either into bug reports.
