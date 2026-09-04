#!/usr/bin/env bash
# Create or reuse the orchestrator's GitHub label and Project board,
# idempotently. Identity comes from PLONK_AGENT_ID / FLEET_AGENT_NAME —
# the exact env names the fleet binary reads for `fleet board sync`.
set -euo pipefail

if [ -f .env ]; then
    set -a
    # shellcheck disable=SC1091
    source .env
    set +a
fi

agent_id=${PLONK_AGENT_ID:-}
agent_name=${FLEET_AGENT_NAME:-}

if [[ -z "$agent_id" || -z "$agent_name" ]]; then
    echo "PLONK_AGENT_ID and FLEET_AGENT_NAME are required; ask the user and make no board or identity changes." >&2
    exit 2
fi

if [[ ! "$agent_id" =~ ^[a-z0-9]+(-[a-z0-9]+)*$ ]]; then
    echo "PLONK_AGENT_ID must match [a-z0-9]+(-[a-z0-9]+)*." >&2
    exit 2
fi

if [[ "$agent_name" =~ ^[[:space:]] || "$agent_name" =~ [[:space:]]$ ]]; then
    echo "FLEET_AGENT_NAME must be non-empty with no leading or trailing whitespace." >&2
    exit 2
fi

if [[ ${1:-} == "--check-identity" ]]; then
    exit 0
fi

repo=${FLATBED_GITHUB_REPOSITORY:-plonklabs/flatbed}
owner=${repo%%/*}
label="orchestrator:$agent_id"
# The board title is fleet's convention — `fleet board sync` looks the
# project up by exactly this name.
board_title="Plonk Board — $agent_name"

update_single_select() {
    local field_id=$1
    local field_name=$2
    local spec=$3
    local fields=$4
    local current_names expected_names options mutation

    current_names=$(jq -c --arg field "$field_name" \
        '[.fields[] | select(.name == $field) | .options[].name] | sort' <<<"$fields")
    expected_names=$(jq -c '[.[].name] | sort' <<<"$spec")
    if [[ "$current_names" == "$expected_names" ]]; then
        return
    fi

    options=$(jq -r --arg field "$field_name" --argjson spec "$spec" '
      [.fields[] | select(.name == $field) | .options] | first as $existing
      | $spec
      | map(. as $wanted
          | ([$existing[] | select(.name == $wanted.name) | .id] | first) as $id
          | "{" +
            (if $id then "id:" + ($id | @json) + "," else "" end) +
            "name:" + ($wanted.name | @json) + "," +
            "color:" + $wanted.color + "," +
            "description:" + ($wanted.description | @json) + "}")
      | join(",")
    ' <<<"$fields")
    mutation="mutation { updateProjectV2Field(input: { fieldId: \"$field_id\", singleSelectOptions: [$options] }) { projectV2Field { ... on ProjectV2SingleSelectField { id } } } }"
    gh api graphql -f query="$mutation" >/dev/null
}

gh label create "$label" --repo "$repo" --color 1D76DB \
    --description "Managed by the $agent_name orchestrator" --force >/dev/null

mk() { gh label create "$1" --repo "$repo" --color "$2" --description "$3" --force >/dev/null; }
mk "worker:🤖flatbed1" 1D76DB "Owned by the flatbed1 worktree session"
mk "worker:🤖flatbed2" 1D76DB "Owned by the flatbed2 worktree session"
mk "worker:🤖flatbed3" 1D76DB "Owned by the flatbed3 worktree session"
mk "✅ ready"          0E8A16 "Refined; an agent can take it end-to-end"
mk "🔍 needs-refinement" C5DEF5 "Not yet scoped to a dispatchable PR"
mk "📦 epic"           5319E7 "Multi-PR feature tracker"
mk "state:⏳queued"    CCCCCC "Accepted for dispatch; no free worker slot yet"
mk "state:🔨active"    FBCA04 "Worker is coding / PR open / in review"
mk "state:🛑blocked"   B60205 "Needs the user (question, decision, or failure)"

projects=$(gh project list --owner "$owner" --limit 1000 --format json)
project_number=$(jq -r --arg title "$board_title" \
    'first(.projects[] | select(.title == $title) | .number) // empty' <<<"$projects")

if [[ -z "$project_number" ]]; then
    project_number=$(gh project create --owner "$owner" --title "$board_title" \
        --format json --jq '.number')
fi

fields=$(gh project field-list "$project_number" --owner "$owner" --format json)
status_id=$(jq -r '.fields[] | select(.name == "Status") | .id' <<<"$fields")
# Full fleet Status vocabulary; flatbed has no test bench so 🚦/🧪 stay
# unused, but keeping them means `fleet board sync` never meets a missing
# column.
status_spec='[
  {"name":"📋 ready","color":"GRAY","description":"Ready for dispatch"},
  {"name":"⏳ queued","color":"GRAY","description":"Accepted; awaiting a worker slot"},
  {"name":"🔨 active","color":"YELLOW","description":"Implementation or review in progress"},
  {"name":"🚦 waiting","color":"ORANGE","description":"Unused in flatbed (no test bench)"},
  {"name":"🧪 testing","color":"PURPLE","description":"Unused in flatbed (no test bench)"},
  {"name":"🛑 blocked","color":"RED","description":"Needs user input"},
  {"name":"✅ done","color":"GREEN","description":"Merged and closed"}
]'

if [[ -z "$status_id" ]]; then
    echo "GitHub Project $project_number has no Status field." >&2
    exit 1
fi

update_single_select "$status_id" Status "$status_spec" "$fields"

fields=$(gh project field-list "$project_number" --owner "$owner" --format json)
slot_id=$(jq -r '.fields[] | select(.name == "Slot") | .id' <<<"$fields")
if [[ -z "$slot_id" ]]; then
    gh project field-create "$project_number" --owner "$owner" --name Slot \
        --data-type SINGLE_SELECT \
        --single-select-options 'flatbed1,flatbed2,flatbed3,user' >/dev/null
else
    slot_spec='[
      {"name":"flatbed1","color":"BLUE","description":"Worker slot 1"},
      {"name":"flatbed2","color":"BLUE","description":"Worker slot 2"},
      {"name":"flatbed3","color":"BLUE","description":"Worker slot 3"},
      {"name":"user","color":"PINK","description":"User-owned slot"}
    ]'
    update_single_select "$slot_id" Slot "$slot_spec" "$fields"
fi

printf '%s\n' "$project_number"
