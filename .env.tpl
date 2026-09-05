# Orchestrator identity — copy to .env and fill in. These are the env names
# the fleet binary reads; scripts source .env when present.
#
# Stable routing key: [a-z0-9]+(-[a-z0-9]+)*; names the orchestrator:<id> label.
PLONK_AGENT_ID=
# Display name; names the "Plonk Board — <name>" GitHub Project.
FLEET_AGENT_NAME=
# Target repository for every fleet GitHub read and write. Fleet defaults to
# plonklabs/plonk, so without this a board sync reports "in sync" after
# projecting nothing and `fleet merge` dies on a 404 for a PR number that
# means something else in the other repo.
PLONK_GITHUB_REPOSITORY=plonklabs/flatbed
