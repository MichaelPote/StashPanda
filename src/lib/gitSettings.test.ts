import { describe, expect, it } from "vitest";
import type { ConfigEntry } from "@/lib/bindings";
import { baseSshCommand, catalogueSetting, describeValue, otherEntries, parseBool, settingValues, sshCommandWithKey, sshKeyOf, sshKeyProblem } from "./gitSettings";

const entry = (scope: string, key: string, value: string | null): ConfigEntry => ({ scope, origin: `file:${scope}`, key, value });

describe("a setting's value in each file", () => {
  // As Git lists them: keys in lower case, files in the order Git reads them.
  const entries = [
    entry("system", "push.default", "matching"),
    entry("global", "push.followtags", "true"),
    entry("global", "push.default", "current"),
    entry("local", "push.default", "upstream"),
    entry("local", "core.flag", null),
  ];

  it("shows a repository's own value, and what it would inherit from the global and system files", () => {
    expect(settingValues(entries, "push.default", "local")).toEqual({
      own: { scope: "local", value: "upstream" },
      inherited: { scope: "global", value: "current" },
    });
  });

  it("shows the global file's value, inheriting only from the system file", () => {
    expect(settingValues(entries, "push.default", "global")).toEqual({
      own: { scope: "global", value: "current" },
      inherited: { scope: "system", value: "matching" },
    });
  });

  it("compares keys without case, and says when nothing sets a key", () => {
    expect(settingValues(entries, "push.followTags", "local")).toEqual({ own: { scope: null, value: null }, inherited: { scope: "global", value: "true" } });
    expect(settingValues(entries, "rerere.enabled", "local").inherited).toEqual({ scope: null, value: null });
  });

  it("reads a key with no value as true", () => {
    expect(settingValues(entries, "core.flag", "local").own.value).toBe("true");
  });

  it("lists only the keys the catalogue does not explain under Other settings", () => {
    expect(otherEntries(entries, "local").map((e) => e.key)).toEqual(["core.flag"]);
    expect(otherEntries(entries, "global")).toEqual([]);
  });
});

describe("reading values", () => {
  it("accepts every way Git writes a boolean", () => {
    for (const v of ["true", "Yes", "on", "1", ""]) expect(parseBool(v)).toBe(true);
    for (const v of ["false", "NO", "off", "0"]) expect(parseBool(v)).toBe(false);
    expect(parseBool("maybe")).toBeNull();
  });

  it("describes values the way the dialog shows them", () => {
    expect(describeValue(catalogueSetting("PUSH.FOLLOWTAGS"), "yes")).toBe("On");
    expect(describeValue(catalogueSetting("pull.rebase"), "merges")).toBe("Rebase, keeping the branch's own merge commits");
    expect(describeValue(catalogueSetting("user.name"), "Ada")).toBe("Ada");
    expect(describeValue(undefined, "x")).toBe("x");
  });
});

describe("the SSH command and its key", () => {
  it("is a setting the dialog explains, not one of the Other settings", () => {
    expect(catalogueSetting("core.sshcommand")?.key).toBe("core.sshCommand");
    expect(otherEntries([entry("local", "core.sshcommand", "ssh -i /k")], "local")).toEqual([]);
  });

  it("finds the key file a command names", () => {
    expect(sshKeyOf('ssh -i "C:/Users/me/.ssh/work key" -o IdentitiesOnly=yes')).toBe("C:/Users/me/.ssh/work key");
    expect(sshKeyOf("ssh -i ~/.ssh/id_ed25519")).toBe("~/.ssh/id_ed25519");
    expect(sshKeyOf("ssh -o IdentitiesOnly=yes -i'/home/me/.ssh/a b'")).toBe("/home/me/.ssh/a b");
    // The last one, which is the one StashPanda adds after whatever was there.
    expect(sshKeyOf('ssh -i /first -F /dev/null -i "/second" -o IdentitiesOnly=yes')).toBe("/second");
  });

  it("builds the command for a chosen key, on Git's plain ssh or on the command already there", () => {
    expect(sshCommandWithKey("", "C:/Users/me/.ssh/work")).toBe('ssh -i "C:/Users/me/.ssh/work" -o IdentitiesOnly=yes');
    expect(sshCommandWithKey("C:/Windows/System32/OpenSSH/ssh.exe", "/home/me/.ssh/a key")).toBe(
      'C:/Windows/System32/OpenSSH/ssh.exe -i "/home/me/.ssh/a key" -o IdentitiesOnly=yes',
    );
  });

  it("replaces a key chosen before, and leaves a command written by hand as it is", () => {
    const first = sshCommandWithKey("ssh -F /etc/ssh/other_config", "/home/me/.ssh/a");
    expect(baseSshCommand(first)).toBe("ssh -F /etc/ssh/other_config");
    expect(sshCommandWithKey(first, "/home/me/.ssh/b")).toBe('ssh -F /etc/ssh/other_config -i "/home/me/.ssh/b" -o IdentitiesOnly=yes');
    const byHand = "ssh -i ~/.ssh/mine -o IdentitiesOnly=yes";
    expect(baseSshCommand(byHand)).toBe(byHand);
    expect(sshKeyOf(sshCommandWithKey(byHand, "/k"))).toBe("/k");
  });

  it("refuses a key file whose path would break the command", () => {
    expect(sshKeyProblem("C:/Users/me/.ssh/id (work)")).toBeNull();
    expect(sshKeyProblem("/home/me/.ssh/id_ed25519")).toBeNull();
    for (const bad of ['/k"x', "/k$(id)", "/k`id`", "C:\\Users\\me\\.ssh\\id", "/k\nx"]) expect(sshKeyProblem(bad), bad).not.toBeNull();
  });

  it("finds none in a command without one", () => {
    expect(sshKeyOf("ssh")).toBeNull();
    expect(sshKeyOf("C:/Windows/System32/OpenSSH/ssh.exe -o IdentitiesOnly=yes")).toBeNull();
    expect(sshKeyOf("plink -batch")).toBeNull();
    expect(sshKeyOf("")).toBeNull();
  });
});
