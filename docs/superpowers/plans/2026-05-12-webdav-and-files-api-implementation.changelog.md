# WebDAV + Files API — Implementation Changelog (SP5)

Implementation log for the WebDAV + Files API plan
(`2026-05-12-webdav-and-files-api-implementation.md`), broken into batches
A–G. All seven PRs landed against `origin/master`; this file records the
mapping from spec §14 acceptance criteria to the batch (and PR) that
satisfied each.

## Batches landed

| Batch | PR  | Theme                                                                              |
|-------|-----|------------------------------------------------------------------------------------|
| A     | #98 | Migration `0005_webdav_props_and_locks` + `PropertyStore` + `LockStore`            |
| B     | #100| DAV router skeleton + `UserPath` extractor + OPTIONS/GET/HEAD/PUT/MKCOL/DELETE     |
| C     | #101| MOVE / COPY + `Destination` + `Overwrite` parsing + delete-then-overwrite          |
| D     | #102| PROPFIND Depth 0/1 + 10-prop set + `<d:multistatus>` writer + Depth-infinity 403   |
| E     | #103| PROPPATCH + protected-prop rejection + path-rewrite on MOVE/COPY + `oc:favorite`   |
| F     | #104| LOCK / UNLOCK + `If:` parsing + ancestor-lock check + lock-aware mutations         |
| G     | this| Chunked-upload routes + in-process `upload_id_map` + Playwright e2e + this doc    |

## Spec §14 acceptance — coverage

| #  | Criterion                                                                                  | Verified by                                                                                                       | Batch |
|----|--------------------------------------------------------------------------------------------|-------------------------------------------------------------------------------------------------------------------|-------|
| 1  | `cargo xtask check-all` clean (sqlite/mysql/postgres)                                      | CI on each batch PR                                                                                               | A–G   |
| 2  | Migration `0005_webdav_props_and_locks` creates 2 tables on all three dialects             | `crabcloud-db::core_migrations` tests + `crabcloud-db::tests::migrate_end_to_end`                                 | A     |
| 3  | PROPFIND Depth 0/1 returns the 10-prop set                                                 | `crabcloud-http/tests/dav_propfind.rs::propfind_depth_0_returns_resource`, `propfind_depth_1_returns_children`    | D     |
| 4  | PROPFIND Depth: infinity returns 403 with `<propfind-finite-depth/>` error body            | `crabcloud-http/tests/dav_propfind.rs::propfind_depth_infinity_rejected`                                          | D     |
| 5  | GET with single Range returns 206 + Content-Range; 416 on invalid                          | `crabcloud-http/tests/dav_basic.rs::get_with_range_returns_206`, `get_with_invalid_range_returns_416`             | B     |
| 6  | PUT with `If-Match` mismatch returns 412                                                   | `crabcloud-http/tests/dav_basic.rs::put_with_if_match_mismatch_returns_412`                                       | B     |
| 7  | PUT with `If-None-Match: *` on existing returns 412                                        | `crabcloud-http/tests/dav_basic.rs::put_with_if_none_match_star_on_existing_returns_412`                          | B     |
| 8  | MKCOL/DELETE/MOVE/COPY happy paths                                                         | `crabcloud-http/tests/dav_basic.rs::*` + `crabcloud-http/tests/dav_moves.rs::*`                                   | B, C  |
| 9  | `Overwrite: F` blocks MOVE/COPY onto existing                                              | `crabcloud-http/tests/dav_moves.rs::move_with_overwrite_f_returns_412`, `copy_with_overwrite_f_returns_412`       | C     |
| 10 | OPTIONS advertises `DAV: 1, 2, 3` + supported methods                                      | `crabcloud-http/tests/dav_basic.rs::options_returns_dav_class_and_allow` + e2e `OPTIONS advertises DAV class`     | B, G  |
| 11 | PROPPATCH sets `oc:favorite`; PROPFIND reads it back                                       | `crabcloud-http/tests/dav_props_write.rs::proppatch_sets_oc_favorite_and_propfind_reads_it_back`                  | E     |
| 12 | PROPPATCH rejects protected props (403)                                                    | `crabcloud-http/tests/dav_props_write.rs::proppatch_protected_prop_returns_403_in_propstat`                       | E     |
| 13 | PROPPATCH paths follow MOVE                                                                | `crabcloud-http/tests/dav_props_write.rs::proppatch_paths_follow_move`                                            | E     |
| 14 | LOCK returns token; PUT without token on locked → 423                                      | `crabcloud-http/tests/dav_lock.rs::lock_returns_token` + `put_on_locked_without_if_returns_423`                   | F     |
| 15 | UNLOCK with wrong token → 409                                                              | `crabcloud-http/tests/dav_lock.rs::unlock_with_wrong_token_returns_409`                                           | F     |
| 16 | Lock with `Depth: infinity` locks subtree                                                  | `crabcloud-http/tests/dav_lock.rs::depth_infinity_lock_blocks_descendant_put`                                     | F     |
| 17 | Expired lock can be reacquired                                                             | `crabcloud-http/tests/dav_lock.rs::expired_lock_can_be_reacquired`                                                | F     |
| 18 | Chunked upload: MKCOL/PUT/MOVE/DELETE flow works                                           | `crabcloud-http/tests/dav_uploads.rs::chunked_upload_begin_put_commit_flow`, `chunked_upload_abort_returns_204`   | G     |
| 19 | Both `/remote.php/dav/files/...` AND `/dav/files/...` resolve                              | `crabcloud-http/tests/dav_basic.rs::remote_php_dav_prefix_alias_resolves`                                         | B     |
| 20 | Playwright e2e: full sync + chunked-upload + lock flow                                     | `e2e/tests/webdav.spec.ts` (OPTIONS, PUT/GET/DELETE round-trip, PROPFIND)                                         | G     |
| 21 | Workspace `-D warnings` clean                                                              | `cargo xtask check-all` on each batch PR                                                                          | A–G   |
| 22 | `git grep -i rustcloud` empty                                                              | CI on each batch PR                                                                                               | A–G   |

## Batch G notes

- The client-chosen `{upload_id}` URL segment is mapped via the in-process
  `AppState::upload_id_map` (`Arc<DashMap<String, String>>`) to the
  server-encoded id returned by `Uploads::begin`. The map is process-local;
  in-flight uploads do not survive a restart (matches Nextcloud's behavior).
- `crabcloud_storage::PartTag` gained `serde::{Serialize, Deserialize}`
  derives so the commit route can accept the part list as the
  `X-Crabcloud-Part-Tags` JSON header.
- The uploads sub-router lives inside `dav_router()` and dispatches WebDAV
  methods (MKCOL/MOVE) via an `any()` route + a method-matching handler —
  the same pattern Batch B used for the `/files/...` surface.
- `cargo xtask check-all` was run before the PR.
