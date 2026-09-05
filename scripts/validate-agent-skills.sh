#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

python3 - <<'PY'
from pathlib import Path
import re
import subprocess
import tempfile

skills = Path('.agents/skills')
if not skills.is_dir():
    raise SystemExit('missing canonical skills directory')


def parse_skill(path, expected):
    text = path.read_text()
    match = re.match(r'^---\n(.*?)\n---\n', text, re.S)
    if not match:
        raise SystemExit(f'{path}: missing YAML frontmatter')
    fields = {}
    for line in match.group(1).splitlines():
        if line == 'disable-model-invocation: true':
            continue
        parsed = re.fullmatch(r'(name|description):[ ]+(.+)', line)
        if not parsed:
            raise SystemExit(f'{path}: malformed frontmatter line')
        key, value = parsed.groups()
        if key in fields:
            raise SystemExit(f'{path}: duplicate frontmatter key {key!r}')
        if value.startswith(('"', "'")) or value.endswith(('"', "'")) or ':' in value:
            raise SystemExit(f'{path}: unsupported plain-scalar YAML value')
        if value.lower() in {'true', 'false', 'null', '~'} or re.fullmatch(r'[-+]?\d+(?:\.\d+)?', value) or value.startswith(('[', '{')):
            raise SystemExit(f'{path}: metadata value must be a string scalar')
        fields[key] = value
    if set(fields) != {'name', 'description'}:
        raise SystemExit(f'{path}: frontmatter must contain exactly name and description')
    if fields['name'] != expected:
        raise SystemExit(f'{path}: metadata name must be {expected!r}')
    if not re.fullmatch(r'[a-z0-9]+(?:-[a-z0-9]+)*', fields['name']) or len(fields['name']) > 64:
        raise SystemExit(f'{path}: invalid Agent Skills name')
    if not fields['description'] or len(fields['description']) > 300:
        raise SystemExit(f'{path}: description must be concise and non-empty')
    if '\n' not in text[match.end():].strip():
        raise SystemExit(f'{path}: missing skill instructions')


for path in sorted(skills.glob('*/SKILL.md')):
    parse_skill(path, path.parent.name)

with tempfile.TemporaryDirectory() as tmp:
    root = Path(tmp)
    for label, frontmatter, closed in (
        ('unclosed', 'name: broken\ndescription: missing close', False),
        ('mismatch', 'name: other\ndescription: valid', True),
        ('malformed-quote', 'name: "broken\ndescription: valid', True),
        ('duplicate-key', 'name: one\nname: two\ndescription: valid', True),
        ('invalid-colon', 'name: one\ndescription: bad: yaml', True),
        ('boolean-value', 'name: one\ndescription: true', True),
        ('list-value', 'name: one\ndescription: [bad]', True),
        ('unexpected-key', 'name: one\nextra: value\ndescription: valid', True),
        ('bad-name', 'name: Bad_Name\ndescription: valid', True),
    ):
        fixture = root / f'{label}.md'
        fixture.write_text(f'---\n{frontmatter}\n---\n\nbody\n' if closed else f'---\n{frontmatter}\n')
        try:
            parse_skill(fixture, 'one' if label not in ('unclosed', 'mismatch') else label)
        except SystemExit:
            continue
        raise SystemExit(f'{label} frontmatter fixture unexpectedly passed')

# The Claude adapter discovers skills through symlinks into the canonical
# directory; a copied directory silently forks the two.
for path in sorted(Path('.claude/skills').glob('*/SKILL.md')):
    if not path.parent.is_symlink():
        raise SystemExit(f'{path}: Claude skill directory must be a symlink')
    if path.parent.resolve() != (skills / path.parent.name).resolve():
        raise SystemExit(f'{path}: symlink does not target canonical skill directory')

for required in ('implement', 'orchestrator', 'pr', 'review', 'spec', 'work-status'):
    if not (skills / required / 'SKILL.md').is_file():
        raise SystemExit(f'missing required canonical skill: {required}')

# Semantic contract checks: each skill must still carry its operational
# content, so a rewrite that drops a load-bearing verb fails here rather than
# on the next dispatch.
required_terms = {
    'spec': ('## Steps', 'Acceptance Criteria'),
    'implement': ('pure-cleanup', 'fleet heartbeat --issue <n>', 'fleet merge <n> --no-merge'),
    'pr': ('Closes #', 'Part of #'),
    'work-status': ('orchestrator:$PLONK_AGENT_ID',),
    'orchestrator': (
        'fleet brief --issue',
        'fleet brief --rewake --issue',
        'its only output is a\n  re-wake',
        '**Relay, not driver.**',
        '**Unblock.**',
        '**Reclaim.**',
    ),
}
for name, terms in required_terms.items():
    body = (skills / name / 'SKILL.md').read_text()
    for term in terms:
        if term not in body:
            raise SystemExit(f'{name}: missing semantic contract {term!r}')

# Dispatch prose regression: the orchestrator renders spawn and re-wake
# payloads through `fleet brief`; the hand-composed contract it replaced must
# not come back.
orchestrator_body = (skills / 'orchestrator' / 'SKILL.md').read_text()
for phrase in ('**Spawn payload**', 'always in four sections', 'merge-authority mode for this dispatch'):
    if phrase in orchestrator_body:
        raise SystemExit(f'orchestrator: hand-composed dispatch prose reintroduced: {phrase!r}')

# Wiring checks: skill bodies name a workflow file, a bot login, and an
# owner/repo pair. Prose substitution silently breaks all three (a poll on a
# workflow that does not exist, a thread filter on a login that never posts, a
# query against the wrong repository), so each is checked against the
# repository itself rather than against a constant that would be substituted
# along with the prose.
workflows = {p.name for p in Path('.github/workflows').glob('*.yml')}
review_workflows = sorted(w for w in workflows if w.endswith('-review.yml'))
bot_login = review_workflows[0][:-len('-review.yml')] if review_workflows else None
try:
    origin = subprocess.run(['git', 'remote', 'get-url', 'origin'], capture_output=True, text=True, check=True).stdout.strip()
    origin_pair = re.search(r'github\.com[:/]([^/:]+)/([^/]+?)(?:\.git)?$', origin)
    owner_repo = origin_pair.groups() if origin_pair else None
except (subprocess.CalledProcessError, FileNotFoundError):
    owner_repo = None


def check_wiring(path, body):
    for wf in re.findall(r'--workflow=([A-Za-z0-9_.-]+\.yml)', body):
        if wf not in workflows:
            raise SystemExit(f'{path}: polls workflow {wf!r}, which is not in .github/workflows')
    logins = set(re.findall(r'login == "([^"]+)"', body))
    logins |= set(re.findall(r'`([A-Za-z0-9_-]+)\[bot\]`', body))
    for login in logins:
        if bot_login and login.removesuffix('[bot]') != bot_login:
            raise SystemExit(f'{path}: filters on bot login {login!r}; the review workflow is {review_workflows[0]!r}')
    pairs = set(re.findall(r'repos/([A-Za-z0-9_.-]+)/([A-Za-z0-9_.-]+)/', body))
    for pair in pairs:
        if '{' in pair[0] or '$' in pair[0]:
            continue
        if owner_repo and pair != owner_repo:
            raise SystemExit(f'{path}: addresses {"/".join(pair)}; origin is {"/".join(owner_repo)}')


for path in sorted(skills.glob('*/SKILL.md')):
    check_wiring(path, path.read_text())

for label, body in {
    'missing-workflow': 'gh run list --workflow=codex-review.yml',
    'wrong-login': 'select(.author.login == "codex")',
    'wrong-repo': 'gh api repos/plonklabs/other/pulls/1',
}.items():
    if label == 'wrong-repo' and not owner_repo:
        continue
    try:
        check_wiring(Path(label), body)
    except SystemExit:
        continue
    raise SystemExit(f'{label} wiring fixture unexpectedly passed')

# GitHub reads and writes go over REST: pinned in orchestrator and implement,
# and enforced against every skill that names gh commands. The forbidden
# subcommands wrap GraphQL; parallel seats polling them exhaust the shared
# user's GraphQL secondary rate limit and stall every merge behind it.
REST_ONLY_SENTENCE = (
    "Every GitHub read or write goes over REST (`gh api repos/{owner}/{repo}/...`); "
    "the only non-REST calls allowed anywhere in this repo are `gh pr ready` (once "
    "per PR) and the authorized `gh pr merge <n> --squash --admin "
    "--match-head-commit <sha>` that step two of the sanctioned merge path issues"
)
for name in ('orchestrator', 'implement'):
    normalized = re.sub(r'\s+', ' ', (skills / name / 'SKILL.md').read_text())
    if REST_ONLY_SENTENCE not in normalized:
        raise SystemExit(f'{name}: missing REST-only sentence')

FORBIDDEN_GH_PATTERNS = [
    (re.compile(r'\bgh\s+pr\s+view\b'), 'gh pr view'),
    (re.compile(r'\bgh\s+pr\s+checks\b'), 'gh pr checks'),
    (re.compile(r'\bgh\s+pr\s+list\b'), 'gh pr list'),
    (re.compile(r'\bgh\s+issue\s+(list|view)\b'), 'gh issue list/view'),
    (re.compile(r'\bgh\s+issue\s+create\b'), 'gh issue create'),
    (re.compile(r'\bgh\s+run\s+view\b[^\n`]*--json'), 'gh run view --json'),
    (re.compile(r'\bgh\s+repo\s+view\b'), 'gh repo view'),
    (re.compile(r'\bgh\b[^\n`]*--watch\b'), '--watch'),
]


def check_forbidden_gh(path, body):
    # A paragraph that declares the policy ("Forbidden: `gh pr view`, ...")
    # legitimately names every forbidden form; only other paragraphs are
    # checked for an actual (undeclared) usage.
    for paragraph in body.split('\n\n'):
        if 'Forbidden:' in paragraph:
            continue
        for pattern, label in FORBIDDEN_GH_PATTERNS:
            if pattern.search(paragraph):
                raise SystemExit(f'{path}: forbidden gh subcommand {label!r} in skill body')


for path in sorted(skills.glob('*/SKILL.md')):
    check_forbidden_gh(path, path.read_text())

for label, body, should_fail in (
    ('forbidden-pr-view', 'Fetch state with `gh pr view <n> --json state`.', True),
    ('forbidden-declaration-allowed', 'Forbidden: `gh pr view`, `gh pr checks`, `gh pr list`.', False),
    ('forbidden-run-view-json', 'gh run view <id> --json status,conclusion', True),
    ('forbidden-watch', 'gh pr checks 12 --watch', True),
):
    if not should_fail:
        check_forbidden_gh(Path(label), body)
        continue
    try:
        check_forbidden_gh(Path(label), body)
    except SystemExit:
        continue
    raise SystemExit(f'{label} forbidden-gh fixture unexpectedly passed')

print(f'validated {len(list(skills.glob("*/SKILL.md")))} canonical skills')
PY
