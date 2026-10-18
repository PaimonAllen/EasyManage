import assert from "node:assert/strict";
import { test } from "node:test";
import { clearMachineAuthentication, serializeMachineRequest, type MachineAuthentication } from "../src/lib/machineAuthentication.ts";

test("only the selected authentication method is serialized, then caller fields are cleared", () => {
  const password = {host: "10.0.0.9", auth_method: "password" as const, password: "fixture-password", sudo_password: "fixture-sudo", deployment_mode: "full"};
  const first = JSON.parse(serializeMachineRequest(password));
  assert.equal(first.password, "fixture-password");
  assert.equal(first.private_key, undefined);
  assert.equal(password.password, "");
  assert.equal(first.sudo_password, "fixture-sudo");
  assert.equal(first.deployment_mode, "full");
  assert.equal(password.sudo_password, "");
  const key = {auth_method: "private_key" as const, private_key: "fixture-key", passphrase: "fixture-phrase"};
  const second = JSON.parse(serializeMachineRequest(key));
  assert.equal(second.private_key, "fixture-key");
  assert.equal(second.passphrase, "fixture-phrase");
  assert.equal(second.password, undefined);
  assert.equal(key.private_key, "");
  assert.equal(key.passphrase, "");
});

test("secret fields are cleared on serialization errors too", () => {
  const value: MachineAuthentication & {circular?: unknown} = {auth_method: "private_key", private_key: "fixture-key", passphrase: "fixture-phrase", sudo_password: "fixture-sudo"};
  value.circular = value;
  assert.throws(() => serializeMachineRequest(value));
  assert.equal(value.private_key, "");
  assert.equal(value.passphrase, "");
  assert.equal(value.sudo_password, "");
  clearMachineAuthentication(value);
});

test("saved credential reuse serializes only the selector, never plaintext secret fields", () => {
  const request: {use_saved: boolean} & Partial<MachineAuthentication> = {use_saved:true};
  assert.deepEqual(JSON.parse(serializeMachineRequest(request)),{use_saved:true});
  assert.deepEqual(request,{use_saved:true});
});
