+++
title = "What a post actually is"
created_at = 2026-09-04T16:20:00Z
+++

One `app/feed/entry` entity: an author, a timestamp, and a body that is an
embed node — a media type plus a payload. Short bodies travel inline. Long ones
become a pointer to a content-addressed blob, because the inline arm is capped
and a post over the cap has exactly one conformant way to be carried.

The entry's key is its own content hash, which is why every reference to one is
a pin rather than a name. Nothing you point at can be swapped underneath you.
