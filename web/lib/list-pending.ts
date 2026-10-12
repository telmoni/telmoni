// A list on its way: the part of a `group/list` that dims while one of its
// links (`LinkPending`) or controls (`data-pending`) waits on the page it
// asked for, after a moment, so a quick answer does not flicker it. Here
// rather than beside `LinkPending`: a value a client module exports reaches a
// Server Component as a reference, not as the classes.
export const LIST_DIMS_WHILE_PENDING =
  "transition-opacity group-has-data-pending/list:opacity-60 group-has-data-pending/list:delay-150";
