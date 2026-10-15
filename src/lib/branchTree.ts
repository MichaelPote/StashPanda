/**
 * Branches grouped into folders by the parts of their names, as the Branches panel lists them:
 * `feature/email` and `feature/sms` both sit in a `feature` folder, and `fix/login/timeout` in a
 * `login` folder inside a `fix` folder. A branch's row keeps its full name.
 */

/** One row of the tree: a folder, or a branch in it, `depth` folders deep. */
export type BranchTreeRow<T> =
  | {
      kind: "folder";
      /** Names the folder for remembering whether it is open: `keyPrefix` and its path. */
      key: string;
      /** The folder's own part of the names, such as `login`. */
      name: string;
      /** Its path with a slash at the end, such as `fix/login/`: the start of a new branch's name. */
      prefix: string;
      /** How many branches are inside it, at any depth. */
      count: number;
      depth: number;
    }
  | { kind: "branch"; key: string; entry: T; depth: number };

interface Node<T> {
  folders: Map<string, Node<T>>;
  branches: { name: string; entry: T }[];
}

const emptyNode = <T>(): Node<T> => ({ folders: new Map(), branches: [] });

function countOf<T>(node: Node<T>): number {
  let n = node.branches.length;
  for (const child of node.folders.values()) n += countOf(child);
  return n;
}

/**
 * The rows for `entries`, whose names `nameOf` gives: in each folder, its folders first, then its
 * branches, each in name order. A folder for which `isOpen` returns false shows without its rows.
 */
export function branchTreeRows<T>(
  entries: readonly T[],
  nameOf: (entry: T) => string,
  keyPrefix: string,
  isOpen: (key: string) => boolean,
): BranchTreeRow<T>[] {
  const root = emptyNode<T>();
  for (const entry of entries) {
    const name = nameOf(entry);
    const parts = name.split("/");
    let node = root;
    for (const part of parts.slice(0, -1)) {
      let next = node.folders.get(part);
      if (!next) node.folders.set(part, (next = emptyNode<T>()));
      node = next;
    }
    node.branches.push({ name, entry });
  }

  const rows: BranchTreeRow<T>[] = [];
  const walk = (node: Node<T>, path: string, depth: number): void => {
    for (const [name, child] of [...node.folders.entries()].sort((a, b) => a[0].localeCompare(b[0]))) {
      const prefix = `${path}${name}/`;
      const key = `${keyPrefix}${prefix}`;
      rows.push({ kind: "folder", key, name, prefix, count: countOf(child), depth });
      if (isOpen(key)) walk(child, prefix, depth + 1);
    }
    for (const b of [...node.branches].sort((x, y) => x.name.localeCompare(y.name))) {
      rows.push({ kind: "branch", key: b.name, entry: b.entry, depth });
    }
  };
  walk(root, "", 0);
  return rows;
}
