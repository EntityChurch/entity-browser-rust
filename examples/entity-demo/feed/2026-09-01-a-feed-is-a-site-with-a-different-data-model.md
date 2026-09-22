+++
title = "A feed is a site with a different data model"
created_at = 2026-09-01T09:00:00Z
+++

Entities at a peer-scoped tree path, projected to static files, committed to by
a signed root. That sentence describes a site and it describes a feed, and the
only thing that differs between them is what the entities mean.

So a feed needed no new durability model, no second publisher and no second
trust chain. It needed a reader, an emitter, and a row in the list of what a
publish reads out of the tree.
