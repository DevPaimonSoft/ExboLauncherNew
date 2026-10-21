# Local compatibility patch

Upstream: https://github.com/ikatson/rqbit, librqbit-core 9.0.1, Apache-2.0.

Only `TorrentMetaV1File` deserialization is adapted. EXBO torrents use the SHA-1
of each file as its wire `path` (including duplicates), with the unique actual
filename in `true_path`. The storage layout uses `true_path`, while the original
raw info dictionary remains untouched for infohash, metadata exchange, and
piece verification. Standard torrents without `true_path` retain their behavior.

Do not sort or rewrite the EXBO info dictionary to fix duplicate filenames:
doing so changes the swarm infohash. Root application tests cover both behavior
and preservation of the original infohash.
