/** Toggle only partition-detail disclosures in the selected node's map. */
export function setAllPartitionDetails(details: Iterable<{ open: boolean }>, expanded: boolean): void {
  for (const detail of details) detail.open = expanded;
}
