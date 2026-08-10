Read a text file. Every line returns as `N:hh|content` (line number, 2-char hash, content); copy those anchors verbatim into `edit` — they are the only way edits address lines.

Page large files with `offset` (1-based line) and `limit`. Truncated output ends with the exact `nextOffset` to continue from; never guess unseen lines.

An empty file returns `[empty]` — insert content with edit `prepend`/`append`, omitting `pos`.

Set `raw: true` to return plain file content without `N:hh` prefixes; offset, limit, and continuation notices still apply.
