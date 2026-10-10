# Contributing

Contributions are licensed under [Apache-2.0](LICENSE), as section 5 of the license states.
Only submit work you have the right to license that way, and include the license and attribution
required by any third-party code you add.

Before sending a change, run `mise run ci` (see [development](docs/development.md)). Tests must
use local mocks and synthetic data: never commit real cookies, trip keys or private itineraries.
Live tests against Wanderlog stay opt-in.
