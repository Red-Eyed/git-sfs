# git-sfs Agent Skill

A Claude Code agent skill that gives your coding agent full context about git-sfs
without needing internet access.

## Install

Copy the `git-sfs/` directory to your Claude skills folder:

**Global (all projects):**
```bash
cp -r skills/git-sfs ~/.claude/skills/git-sfs
```

**Project-only:**
```bash
cp -r skills/git-sfs .claude/skills/git-sfs
```

Once installed, Claude Code picks up the skill automatically. When you ask about
git-sfs, large file management, or anything involving `.git-sfs/`, the skill loads
and Claude runs `git-sfs llms-txt` to pull the full embedded reference.

## What the skill does

- Loads on demand when git-sfs topics are relevant
- Runs `git-sfs llms-txt` to get the complete reference from the installed binary
- Falls back to the inline summary in `SKILL.md` if the binary is not installed
- Works entirely offline — no network calls needed
