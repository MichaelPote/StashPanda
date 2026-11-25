import { describe, expect, it } from "vitest";
import { hostKeyQuestion } from "./askpass";

describe("hostKeyQuestion", () => {
  it("leaves out the line the dialog's buttons stand in for", () => {
    const prompt =
      "The authenticity of host 'example.com (192.0.2.1)' can't be established.\nED25519 key fingerprint is SHA256:abc.\nThis key is not known by any other names.\nAre you sure you want to continue connecting (yes/no/[fingerprint])? ";
    expect(hostKeyQuestion(prompt)).toBe(
      "The authenticity of host 'example.com (192.0.2.1)' can't be established.\nED25519 key fingerprint is SHA256:abc.\nThis key is not known by any other names.",
    );
  });
});
