#!/usr/bin/env python3
"""Which contract-spec clauses actually have a test behind them.

rust-rewrite-plan §7 states the Phase 7 acceptance gate literally: **every
contract-spec clause maps to a passing assertion**, and contract-spec §15 warns
that clauses without assertions are aspirational. That gate needs an enumeration
to check against, and prose cannot be checked.

So the map is data, and it is self-verifying. Every clause claiming coverage
names the file and a fragment of the assertion that covers it; this script
confirms the fragment is still there. Rename an assertion and the map goes red
rather than quietly lying -- which is the failure mode a hand-maintained
checklist always reaches eventually.

    coverage.py            report, and fail only if a claim has gone stale
    coverage.py --gate     also fail while any clause is UNCOVERED (Phase 7)

The default is deliberately not the gate. Gaps here are known and tracked, not
regressions; making them fail today would only teach people to skip the script.

**What this cannot check.** A substring proves a *mention*, not an assertion.
Writing this map already caught one clause claiming benchmark.py enforced a
regression threshold when it only printed ratios -- the evidence fragment
matched, the guarantee did not exist. The fragment therefore has to name the
failure path (`past the threshold`) rather than the subject matter (`ratio`),
and a status is only worth what the reviewer who set it was worth.
"""

from __future__ import annotations

import argparse
import sys
from dataclasses import dataclass
from pathlib import Path

HARNESS_DIR = Path(__file__).parent
REPO_ROOT = HARNESS_DIR.parent.parent

# How a clause is held down, weakest to strongest.
UNCOVERED = "UNCOVERED"  # nothing tests it
V2_ONLY = "V2-ONLY"  # untestable until the Rust binary exists
DECLARED = "DECLARED"  # enumerated in divergences.py; asserts itself once v2 exists
OBSERVED = "OBSERVED"  # recorded as a v1 baseline; v2 must diverge, so not yet asserted
STRUCTURAL = "STRUCTURAL"  # any change shows up as a manifest or argv diff
ASSERTED = "ASSERTED"  # a named assertion fails if violated

ORDER = [ASSERTED, STRUCTURAL, DECLARED, OBSERVED, V2_ONLY, UNCOVERED]


@dataclass(frozen=True)
class Clause:
    section: str
    statement: str
    status: str
    # File containing the evidence, and a fragment that must appear in it.
    # Both empty for UNCOVERED and V2_ONLY.
    source: str = ""
    evidence: str = ""


CLAUSES = [
    # -- §2 repository layout ------------------------------------------------
    Clause(
        "2",
        "`.git-sfs/` is created with mode 0o755",
        STRUCTURAL,
        "scenarios/01-add-commit.sh",
        "setup_repo",
    ),
    Clause(
        "2",
        "`.git-sfs/cache` target is a canonicalized absolute path",
        STRUCTURAL,
        "run.py",
        "resolved form is registered too",
    ),
    Clause(
        "2",
        "rebinding an existing cache is an error, not an overwrite",
        ASSERTED,
        "../../crates/git-sfs-core/src/ports/local_state.rs",
        "bind_cache_refuses_to_repoint_an_existing_link",
    ),
    Clause(
        "2",
        "rebind compares canonicalized paths, so an equivalent path is a no-op",
        ASSERTED,
        "../../crates/git-sfs-core/src/ports/local_state.rs",
        "bind_cache_rebinding_to_an_equivalent_target_is_a_noop",
    ),
    Clause(
        "2",
        "`.git-sfs/cache` must not be committed",
        ASSERTED,
        "../../test/workflows/lib/scenarios.sh",
        ".git-sfs/cache must be ignored local state",
    ),
    # -- §3 symlink format ---------------------------------------------------
    Clause(
        "3.1",
        "targets are relative and shaped `../.git-sfs/cache/files/sha256/<pp>/<hash>`",
        STRUCTURAL,
        "scenarios/01-add-commit.sh",
        "git_sfs add data",
    ),
    Clause(
        "3.2",
        "absolute targets are rejected",
        ASSERTED,
        "../../crates/git-sfs-core/src/domain/symlink.rs",
        "rejects_absolute_targets",
    ),
    Clause(
        "3.2",
        "targets escaping the cache root are rejected",
        ASSERTED,
        "../../crates/git-sfs-core/src/domain/symlink.rs",
        "rejects_targets_that_escape_the_cache_root",
    ),
    Clause(
        "3.2",
        "uppercase hex in a hash is rejected",
        ASSERTED,
        "../../crates/git-sfs-core/src/domain/symlink.rs",
        "rejects_uppercase_hex_in_the_hash_component",
    ),
    Clause(
        "3.2",
        "prefix component must equal the hash's first two characters",
        ASSERTED,
        "../../crates/git-sfs-core/src/domain/symlink.rs",
        "rejects_a_prefix_that_does_not_match_the_hash",
    ),
    Clause(
        "3.3",
        "mv succeeds when the referenced cache object is absent",
        ASSERTED,
        "../../crates/git-sfs-core/src/exec/mv.rs",
        "moves_a_symlink_even_when_referenced_cache_object_is_absent",
    ),
    Clause(
        "3.3",
        "mv rejects a source that is not a git-sfs symlink",
        ASSERTED,
        "../../crates/git-sfs-core/src/exec/mv.rs",
        "refuses_to_move_a_plain_file_that_is_not_a_git_sfs_symlink",
    ),
    Clause(
        "3.3",
        "a directory destination means move-inside; an existing file is an error",
        ASSERTED,
        "../../crates/git-sfs-core/src/exec/mv.rs",
        "directory_destination_means_move_inside_and_existing_file_is_an_error",
    ),
    # -- §4 cache layout -----------------------------------------------------
    Clause(
        "4",
        "objects live at files/sha256/<prefix>/<hash>",
        STRUCTURAL,
        "scenarios/02-push-pull.sh",
        "CACHE/files",
    ),
    Clause(
        "4.1",
        "write bits are stripped before an object is visible",
        ASSERTED,
        "mode_preservation.py",
        "the repaired object is protected again",
    ),
    Clause(
        "4.1",
        "a writable cache file is treated as unverified, hash-verified, then protected in place",
        ASSERTED,
        "mode_preservation.py",
        "re-protects the object in place",
    ),
    Clause(
        "4.1",
        "no object is ever read-only while mismatching its own hash",
        ASSERTED,
        "cancellation.py",
        "no read-only object mismatches its hash",
    ),
    Clause(
        "4.1",
        "explicit integrity checks catch rot the mode bit hid",
        ASSERTED,
        "mode_preservation.py",
        "detects the rot the mode bit hid",
    ),
    Clause(
        "4.1",
        "v1 trusts a protected-but-rotted object forever",
        OBSERVED,
        "mode_preservation.py",
        "pull against a protected rotted object",
    ),
    Clause(
        "4.2",
        "the final mode is set on the temp file before the rename",
        ASSERTED,
        "../../crates/git-sfs-core/src/ports/store.rs",
        "store_temp_gets_final_mode_before_publish",
    ),
    Clause(
        "4.2",
        "a post-rename hash mismatch removes the published file",
        ASSERTED,
        "../../crates/git-sfs-core/src/ports/store.rs",
        "Nothing is published on mismatch.",
    ),
    Clause(
        "4.2",
        "import --move falls back to copy+remove across filesystems",
        ASSERTED,
        "../../crates/git-sfs-core/src/ports/store.rs",
        "adopt_cross_device_fallback_removes_source_only_after_verified_copy",
    ),
    Clause("4.3", "tmp/ is purged by pull only, before it takes its lock", UNCOVERED),
    Clause(
        "4.3",
        "purging tmp/ must not destroy another process's in-flight staging",
        V2_ONLY,
    ),
    Clause(
        "4",
        "a v2-style trash/ directory is invisible to v1",
        ASSERTED,
        "downgrade.py",
        "unaffected by a trash/ directory",
    ),
    Clause(
        "4",
        "trashed objects keep their read-only bit",
        ASSERTED,
        "downgrade.py",
        "keeps its read-only bit",
    ),
    # -- §5 remote layout ----------------------------------------------------
    Clause(
        "5",
        "remote objects live at <url>/files/sha256/<prefix>/<hash>",
        ASSERTED,
        "downgrade.py",
        "lands at the layout",
    ),
    Clause(
        "5",
        "the rclone argv stream is stable across implementations",
        STRUCTURAL,
        "scenarios/04-rclone-argv.sh",
        "use_fake_rclone",
    ),
    Clause(
        "5.1",
        "backend and path compose to backend:path, trailing slashes stripped",
        ASSERTED,
        "../../crates/git-sfs-core/src/domain/remote.rs",
        "matches_the_contract_spec_url_composition_table",
    ),
    Clause(
        "5.1",
        "a Windows drive-letter path is preserved verbatim",
        ASSERTED,
        "../../crates/git-sfs-core/src/domain/remote.rs",
        '("s3", "D:/data", "s3:D:/data")',
    ),
    Clause(
        "5.1",
        "an empty backend yields the path unchanged",
        ASSERTED,
        "../../crates/git-sfs-core/src/domain/remote.rs",
        '("", "/abs/path", "/abs/path")',
    ),
    # -- §5b operation scope -------------------------------------------------
    Clause(
        "5b",
        "a path argument scopes status, verify, push and pull to a subtree",
        ASSERTED,
        "../../test/workflows/lib/scenarios.sh",
        "path-scoped push leaked a sibling object",
    ),
    Clause(
        "5b",
        "pull <path> does not restore siblings",
        ASSERTED,
        "../../test/workflows/lib/scenarios.sh",
        "selected pull must not restore sibling objects",
    ),
    Clause(
        "5b.1",
        "push names a working-tree path, not a hash, for a missing object",
        ASSERTED,
        "../../crates/git-sfs-core/src/plan/push.rs",
        "fails_naming_the_first_missing_path_in_sorted_order_when_skip_missing_is_false",
    ),
    Clause(
        "5b.1",
        "--skip-missing never uploads an object that fails HasValid",
        ASSERTED,
        "../../crates/git-sfs-core/src/plan/push.rs",
        "skip_missing_uploads_present_objects_and_reports_every_path_referencing_a_skipped_one",
    ),
    Clause(
        "5b.1",
        "--skip-missing reports object and symlink counts",
        ASSERTED,
        "../../crates/git-sfs-core/src/exec/push.rs",
        "skip_missing_uploads_present_objects_and_reports_skipped_objects",
    ),
    Clause(
        "5b.1",
        "the skipped-path listing is capped with an 'and N more' line",
        ASSERTED,
        "../../crates/git-sfs/src/reporting.rs",
        "skipped_push_warning_is_capped_and_keeps_the_repair_hint",
    ),
    Clause(
        "5b.2",
        "a symlinked import source is rejected without -L",
        ASSERTED,
        "../../crates/git-sfs-core/src/exec/import.rs",
        "refuses_a_symlink_source_without_follow_and_leaves_both_link_and_target_untouched",
    ),
    Clause(
        "5b.2",
        "a rejected import leaves the source link and its target intact",
        ASSERTED,
        "../../crates/git-sfs-core/src/exec/import.rs",
        "refuses_a_symlink_source_without_follow_and_leaves_both_link_and_target_untouched",
    ),
    # -- §6 config -----------------------------------------------------------
    Clause(
        "6.2",
        "unknown top-level fields, sections, settings and remote fields are rejected",
        ASSERTED,
        "downgrade.py",
        "is rejected, so v2 must not add one",
    ),
    Clause(
        "6.2",
        "a `cache` field or [cache] section is rejected",
        ASSERTED,
        "../../crates/git-sfs-core/src/domain/config/mod.rs",
        "rejects_a_cache_section",
    ),
    Clause(
        "6.2",
        "`version` absent or not 1 is rejected",
        ASSERTED,
        "../../crates/git-sfs-core/src/domain/config/legacy_scanner.rs",
        "rejects_missing_version",
    ),
    Clause(
        "6.2",
        "`algorithm` other than sha256 is rejected",
        ASSERTED,
        "../../crates/git-sfs-core/src/domain/config/legacy_scanner.rs",
        "rejects_an_unsupported_algorithm",
    ),
    Clause(
        "6.2",
        "negative `n_jobs` is rejected",
        ASSERTED,
        "../../crates/git-sfs-core/src/domain/config/mod.rs",
        "rejects_negative_n_jobs_structurally",
    ),
    Clause(
        "6.2",
        "a remote missing `backend` is rejected",
        ASSERTED,
        "../../crates/git-sfs-core/src/domain/config/mod.rs",
        "rejects_a_remote_missing_backend",
    ),
    Clause(
        "6.3",
        "`#` inside a quoted value truncates under v1's scanner",
        ASSERTED,
        "../../crates/git-sfs-core/src/domain/config/legacy_scanner.rs",
        "strips_the_first_hash_anywhere_including_inside_quotes",
    ),
    Clause("6.5", "both parsers are run and disagreement is an error", V2_ONLY),
    Clause(
        "6.4",
        "the init template parses under the implementation's own validator",
        ASSERTED,
        "../../crates/git-sfs-core/src/domain/config/mod.rs",
        "the_default_template_parses_and_does_not_trip_its_own_ambiguity_check",
    ),
    Clause(
        "6.6",
        "a leading `v` is accepted in a version string",
        ASSERTED,
        "../../crates/git-sfs-core/src/domain/version_floor.rs",
        "a_leading_v_is_stripped",
    ),
    Clause(
        "6.6",
        "a version string requires exactly three numeric components",
        ASSERTED,
        "../../crates/git-sfs-core/src/domain/version_floor.rs",
        "exactly_three_components_are_required",
    ),
    Clause(
        "6.6",
        "v2 is never less permissive than v1 on the version forms v1 accepts",
        ASSERTED,
        "../../crates/git-sfs-core/src/domain/version_floor.rs",
        "leading_zeros_are_accepted",
    ),
    # -- §7 local state ------------------------------------------------------
    Clause(
        "7.1",
        "`.git` as a file (worktree, submodule) is accepted",
        ASSERTED,
        "../../crates/git-sfs-core/src/ports/local_state.rs",
        "choose_cache_root_uses_the_gitdir_pointer_for_worktrees",
    ),
    Clause(
        "7.2",
        "cache binding: init/setup --cache, symlink, else error",
        ASSERTED,
        "../../crates/git-sfs-core/src/ports/local_state.rs",
        "cache_binding_uses_requested_cache_then_symlink_else_error",
    ),
    # -- §8 lock protocol ----------------------------------------------------
    Clause(
        "8",
        "the lock is a directory at locks/<name>.lock, mode 0755",
        ASSERTED,
        "lock_contention.py",
        "lock directory mode is 0755",
    ),
    Clause(
        "8",
        "the owner file holds 'pid: <N>' at mode 0644",
        ASSERTED,
        "lock_contention.py",
        "owner file mode is 0644",
    ),
    Clause(
        "8",
        "the lock is released on completion",
        ASSERTED,
        "lock_contention.py",
        "lock is released when push finishes",
    ),
    Clause(
        "8",
        "a binary blocks on a lock another process created",
        ASSERTED,
        "lock_contention.py",
        "does not proceed while the lock is held",
    ),
    Clause(
        "8",
        "cross-binary contention: the second binary waits for the first",
        ASSERTED,
        "lock_contention.py",
        "genuinely blocked, not racing through",
    ),
    Clause(
        "8",
        "contention polls every 100ms",
        ASSERTED,
        "../../crates/git-sfs-core/src/ports/lock.rs",
        "contention_poll_interval_is_frozen_at_100ms",
    ),
    Clause(
        "8.1",
        "v1 waits forever and panics on a malformed owner file",
        OBSERVED,
        "lock_contention.py",
        "zero-byte owner",
    ),
    Clause(
        "8.2",
        "push takes locks/push.lock, the name another binary looks for",
        ASSERTED,
        "lock_contention.py",
        "locks/push.lock exists while push runs",
    ),
    Clause(
        "8.2",
        "add, import, setup and pull take their own named locks",
        ASSERTED,
        "lock_contention.py",
        "waits for locks/{name}.lock",
    ),
    Clause(
        "8.2",
        "consolidating the five names into one lock is detected",
        ASSERTED,
        "lock_contention.py",
        "every command blocks on its own lock name",
    ),
    # -- §9 exit codes -------------------------------------------------------
    Clause(
        "9", "success vs failure is 0 vs non-zero", STRUCTURAL, "run.py", "outcomes"
    ),
    Clause("9", "SIGINT exits 130", ASSERTED, "cancellation.py", "exits {SIGINT_EXIT}"),
    Clause(
        "9",
        "errors print to stderr with the `git-sfs: ` prefix",
        ASSERTED,
        "cancellation.py",
        "reports on stderr with the git-sfs prefix",
    ),
    Clause(
        "9.1",
        "verify exits non-zero on an integrity failure",
        STRUCTURAL,
        "scenarios/03-corrupt-cache.sh",
        # The flag, not just the label: without --no-check-remote the default
        # remote check exits 2 first and the scenario stops testing corruption
        # while still looking green. That is how it shipped until this fragment
        # was tightened.
        "verify --no-check-remote --with-integrity",
    ),
    Clause(
        "9.1",
        "presence-only verify passes where --with-integrity fails (0 vs 3)",
        STRUCTURAL,
        "scenarios/03-corrupt-cache.sh",
        "The split it exists to pin is 0 vs 3",
    ),
    Clause(
        "9.1",
        "status always exits 0",
        STRUCTURAL,
        "scenarios/05-remote-fault.sh",
        "status_object_denied",
    ),
    Clause(
        "9.1",
        "status without --remote makes no network calls",
        ASSERTED,
        "../../crates/git-sfs-core/src/exec/status.rs",
        "status_without_remote_makes_no_remote_observations",
    ),
    Clause(
        "9.1",
        "a repo whose only finding is orphaned objects exits 0",
        ASSERTED,
        "../../crates/git-sfs-core/src/exec/status.rs",
        "status_ignores_orphaned_cache_objects_and_still_succeeds",
    ),
    Clause(
        "9.1",
        "verify exits 0 after repairing a writable but intact cache object",
        DECLARED,
        "divergences.py",
        "writable-object-is-repaired-not-failed",
    ),
    Clause(
        "4.1",
        "a verified writable object is protected in place, losing its write bits",
        DECLARED,
        "divergences.py",
        "writable-object-is-reprotected",
    ),
    Clause(
        "9.2",
        "verify --check-remote must reject a truncated remote object",
        ASSERTED,
        "../../crates/git-sfs-core/src/exec/verify.rs",
        "verify_check_remote_rejects_a_truncated_remote_object",
    ),
    # -- §10 JSON ------------------------------------------------------------
    Clause(
        "10.2",
        "`remotes --json` shape is frozen",
        ASSERTED,
        "../../crates/git-sfs/src/reporting.rs",
        "remotes_json_shape_matches_the_contract",
    ),
    Clause(
        "10.2",
        "`remotes` must not contact a backend",
        ASSERTED,
        "../../crates/git-sfs-core/src/exec/remotes.rs",
        "lists_remote_configuration_without_contacting_the_backend",
    ),
    # -- §11 release artifacts -----------------------------------------------
    Clause(
        "11",
        "archive naming git-sfs-<version>-<os>-<arch>.tar.gz",
        ASSERTED,
        "../workflows/lib/install.sh",
        "git-sfs-$VERSION-$HOST_OS-$HOST_ARCH.tar.gz",
    ),
    Clause(
        "11",
        "SHA256SUMS accompanies the archives and is verified",
        ASSERTED,
        "../workflows/lib/install.sh",
        "SHA256SUMS",
    ),
    Clause(
        "11",
        "--version stays parseable as the tag",
        ASSERTED,
        "../workflows/lib/install.sh",
        "installed git-sfs version",
    ),
    # -- §13 do-not-reproduce ------------------------------------------------
    Clause(
        "13.1",
        "add must not leave a path with neither file nor symlink",
        ASSERTED,
        "cancellation.py",
        "the source path still resolves",
    ),
    Clause(
        "13.1",
        "import --move must publish before removing the source",
        ASSERTED,
        "../../crates/git-sfs-core/src/exec/import.rs",
        "move_flag_removes_the_source_only_after_the_destination_is_published",
    ),
    Clause(
        "13.1",
        "mv must be cancelable",
        ASSERTED,
        "../../crates/git-sfs-core/src/exec/mv.rs",
        "directory_mv_observes_cancellation_before_moving_tree",
    ),
    Clause(
        "13.2",
        "AtomicCopy must fsync the parent directory",
        ASSERTED,
        "../../crates/git-sfs-core/src/ports/store.rs",
        "persist_store_temp_fsyncs_parent_directory_after_rename",
    ),
    Clause(
        "13.2",
        "temp files must not accumulate inside the object store",
        OBSERVED,
        "cancellation.py",
        "temp files left inside files/sha256",
    ),
    Clause(
        "13.3",
        "a denied remote must not be reported as an empty one",
        OBSERVED,
        "scenarios/05-remote-fault.sh",
        "status_object_denied",
    ),
    Clause(
        "13.3",
        "the disk-space guard must not fail open",
        ASSERTED,
        "../../crates/git-sfs-core/src/plan/disk_space.rs",
        "requires_a_ten_percent_margin_over_what_is_needed",
    ),
    Clause(
        "13.3",
        "remote errors must be classified structurally, not by English text",
        ASSERTED,
        "../../crates/git-sfs-core/src/ports/remote.rs",
        "has_file_returns_an_error_when_the_remote_cannot_be_reached",
    ),
    Clause(
        "13.3",
        "a freshly initialized repo must be able to run verify",
        ASSERTED,
        "../../crates/git-sfs-core/src/exec/verify.rs",
        "freshly_initialized_repo_verifies_without_a_remote_check",
    ),
    Clause(
        "13.4",
        "push must not overwrite a good replica with rotted bytes",
        OBSERVED,
        "mode_preservation.py",
        "push with a protected rotted object",
    ),
    Clause(
        "13.4",
        "push must verify what landed",
        OBSERVED,
        "cancellation.py",
        "object left on the remote after the interrupt",
    ),
    Clause(
        "13.4",
        "retries must be limited to transient failure classes",
        DECLARED,
        "divergences.py",
        "retry-only-transient",
    ),
    Clause(
        "13.4",
        "FileSizes must not be O(entire remote)",
        ASSERTED,
        "../../crates/git-sfs-core/src/ports/remote.rs",
        "file_sizes_lists_requested_prefixes_instead_of_the_entire_remote",
    ),
    Clause(
        "13.4b",
        "defaults must not place data in harm's way",
        ASSERTED,
        "../../crates/git-sfs-core/src/exec/add.rs",
        "default_cache_keeps_unpushed_bytes_out_of_git_clean_reach",
    ),
    Clause(
        "13.5",
        "environmental assumptions are checked where they are chosen",
        ASSERTED,
        "../../crates/git-sfs/src/dispatch.rs",
        "doctor_checks_environmental_assumptions_where_they_are_chosen",
    ),
    Clause(
        "13.7",
        "add must refuse a candidate already tracked by Git",
        ASSERTED,
        "../../crates/git-sfs-core/src/exec/add.rs",
        "refuses_to_convert_a_file_already_tracked_by_git",
    ),
    # -- §7c downgrade -------------------------------------------------------
    Clause(
        "7c",
        "one binary's repo, cache, and remote stay operable by the other",
        ASSERTED,
        "downgrade.py",
        "succeeds against",
    ),
    Clause(
        "7c",
        "an object written by one binary is recoverable by the other",
        ASSERTED,
        "downgrade.py",
        "recovers from",
    ),
    # -- §9b performance -----------------------------------------------------
    Clause(
        "9b",
        "no operation regresses past 1.25x the baseline",
        ASSERTED,
        "benchmark.py",
        "past the threshold",
    ),
]


def verify(clause: Clause) -> str | None:
    """Return a problem description, or None when the claim still holds."""
    if clause.status in (UNCOVERED, V2_ONLY):
        return None
    path = HARNESS_DIR / clause.source
    if not path.is_file():
        return f"missing file {clause.source}"
    if clause.evidence not in path.read_text():
        return f"evidence not found in {clause.source}: {clause.evidence!r}"
    return None


def divergence_problems() -> list[str]:
    """Declarations in divergences.py that no longer match reality."""
    import divergences

    scenarios = sorted(p.stem for p in (HARNESS_DIR / "scenarios").glob("*.sh"))
    return divergences.validate(scenarios)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--gate",
        action="store_true",
        help="also fail while any clause is UNCOVERED (the Phase 7 criterion)",
    )
    parser.add_argument(
        "--list", choices=ORDER, help="print only clauses with this status"
    )
    args = parser.parse_args()

    stale = [(c, problem) for c in CLAUSES if (problem := verify(c))]
    stale += [(None, problem) for problem in divergence_problems()]
    counts = {status: sum(1 for c in CLAUSES if c.status == status) for status in ORDER}

    if args.list:
        for clause in CLAUSES:
            if clause.status == args.list:
                print(f"  §{clause.section:<6} {clause.statement}")
        return

    width = max(len(status) for status in ORDER)
    print(f"contract-spec coverage ({len(CLAUSES)} clauses)\n")
    for status in ORDER:
        print(f"  {status:<{width}}  {counts[status]:>3}")

    if stale:
        print(
            f"\n{len(stale)} stale claim(s) -- the map says covered, the file disagrees:"
        )
        for clause, problem in stale:
            if clause is not None:
                print(f"  §{clause.section} {clause.statement}")
            print(f"    {problem}")
        sys.exit(1)

    uncovered = counts[UNCOVERED] + counts[OBSERVED]
    print(f"\nevery claim verified. {uncovered} clause(s) still short of an assertion.")
    print("run with --list UNCOVERED (or OBSERVED) to see them.")

    if args.gate and uncovered:
        sys.exit(f"gate: {uncovered} clause(s) lack a passing assertion")


if __name__ == "__main__":
    main()
