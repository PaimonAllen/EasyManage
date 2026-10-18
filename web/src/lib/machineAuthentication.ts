export type MachineAuthMethod = "password" | "private_key";
export type MachineDeploymentMode = "basic" | "full";
export type MachineAuthentication = {sudo_password?: string} & (
  | {auth_method: "password"; password: string}
  | {auth_method: "private_key"; private_key: string; passphrase: string});

export const MAX_PRIVATE_KEY_BYTES = 12 * 1024;

export function clearMachineAuthentication(value: Partial<MachineAuthentication>): void {
  if ("sudo_password" in value) value.sudo_password = "";
  if ("password" in value) value.password = "";
  if ("private_key" in value) value.private_key = "";
  if ("passphrase" in value) value.passphrase = "";
}

// JavaScript strings / browser TLS buffers cannot be reliably zeroized. Only
// transmit over HTTPS; do not persist this body in browser storage or logs.
export function serializeMachineRequest<T extends Partial<MachineAuthentication>>(value: T): string {
  try { return JSON.stringify(value); }
  finally { clearMachineAuthentication(value); }
}
