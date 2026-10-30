# lvpm

## Slices, issues and the board

Every slice is an issue under #57, shown on the
[lvpm mission](https://github.com/users/ThatsTheEnd/projects/2) board. The issue
is the work item; the board only adds its Status. How to run a slice is in
[docs/vision/build-slices.md](docs/vision/build-slices.md#how-to-run-a-slice).

When you start a slice:

1. Set its card to **In Progress** (`N` is the issue number):

   ```bash
   N=71
   project_id=PVT_kwHOA_lWOM4Bo99Q
   status_field_id=PVTSSF_lAHOA_lWOM4Bo99QzhnboIQ   # the Status field
   in_progress_option_id=47fc9ee4                   # its "In Progress" option
   item_id=$(gh project item-list 2 --owner ThatsTheEnd --limit 300 --format json \
     -q ".items[] | select(.content.number==$N and .content.repository==\"ThatsTheEnd/lvpm\") | .id")
   gh project item-edit --project-id "$project_id" --id "$item_id" \
     --field-id "$status_field_id" --single-select-option-id "$in_progress_option_id"
   ```

   The ids come from `gh project field-list 2 --owner ThatsTheEnd --format json`;
   re-read them there if the edit is refused. Raise `--limit` once the board
   holds more than 300 items.

2. Then follow [How to run a slice](docs/vision/build-slices.md#how-to-run-a-slice):
   the PR says `Closes #N` and, for a `deviation`, how it differs. Merging
   closes the issue and the board moves the card to Done; never close a slice
   issue or set Done by hand.

Work that is not a slice needs no issue; its PR just doesn't say `Closes`.
