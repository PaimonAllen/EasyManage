export type MachineAuthMethod = "password" | "private_key";
export type MachineAuthentication =
  | {auth_method: "password"; password: string}
  | {auth_method: "private_key"; private_key: string; passphrase: string};

export const MAX_PRIVATE_KEY_BYTES = 12 * 1024;

export function clearMachineAuthentication(value: MachineAuthentication): void {
  if (value.auth_method === "password") value.password = "";
  else { value.private_key = ""; value.passphrase = ""; }
}

// JavaScript strings / browser TLS buffers cannot be reliably zeroized. Only
// transmit over HTTPS; do not persist this body in browser storage or logs.
export function serializeMachineRequest<T extends MachineAuthentication>(value: T): string {
  try { return JSON.stringify(value); }
  finally { clearMachineAuthentication(value); }
}
