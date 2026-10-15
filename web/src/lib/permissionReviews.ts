import type { PermissionCheck } from "../types/monitoring";

// Exclude sample time: repeat sampling does not change an accepted configuration.
export function samePermissionCheck(left: PermissionCheck, right: PermissionCheck): boolean {
  return left.category === right.category && left.title === right.title && left.path === right.path
    && left.status === right.status && left.observed === right.observed
    && left.expected === right.expected && left.detail === right.detail;
}
