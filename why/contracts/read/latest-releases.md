# Latest releases

`GET /v0/releases/latest` returns the newest recorded release of every
repository that has a release. The ledger selects each repository's release
by greatest receipt time, breaking ties by descending version text. It then
applies the ordering and paging of the [release history](releases.md).
The cursor may identify a release that has since been superseded.

The compatibility version is in [api.yaml](api.yaml). The
[response schema](releases.schema.json) defines the list entries and cursor.
