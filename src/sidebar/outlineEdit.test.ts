import { describe, expect, it } from "vitest";
import type { OutlineNode } from "../ipc/types";
import {
  countNodes, findNode, flattenEdit, indent, insertAfter, moveBy, moveNode, newId, outdent, removeNode, sameOutline,
  toEditTree, toOutline, updateNode, type EditNode,
} from "./outlineEdit";

const SAMPLE: OutlineNode[] = [
  { title: "A", page: 0, open: true, children: [
    { title: "A.1", page: 1, dest: { y: 700 }, children: [] },
    { title: "A.2", page: 2, children: [] },
  ] },
  { title: "B", page: 3, open: false, children: [{ title: "B.1", page: 4, children: [] }] },
  { title: "Web", page: null, url: "https://example.com/", children: [] },
];

function titles(nodes: EditNode[]): string[] {
  return flattenEdit(nodes).map((r) => `${"  ".repeat(r.depth)}${r.node.title}`);
}

function byTitle(nodes: EditNode[], title: string): EditNode {
  const row = flattenEdit(nodes).find((r) => r.node.title === title);
  if (!row) throw new Error(`no node ${title}`);
  return row.node;
}

describe("outlineEdit", () => {
  it("round-trips the wire tree: open only on nodes with children, url nodes have no page", () => {
    const tree = toEditTree(SAMPLE);
    expect(countNodes(tree)).toBe(6);
    expect(toOutline(tree)).toEqual(SAMPLE);
    // a node that gains children gets an explicit open flag; a url node drops a stray page
    const web = byTitle(tree, "Web");
    const withPage = updateNode(tree, web.id, { page: 3 });
    expect(toOutline(withPage)[2]).toEqual({ title: "Web", page: null, url: "https://example.com/", children: [] });
  });

  it("indents into the previous sibling and outdents after the parent", () => {
    let tree = toEditTree(SAMPLE);
    tree = indent(tree, byTitle(tree, "B").id);
    expect(titles(tree)).toEqual(["A", "  A.1", "  A.2", "  B", "    B.1", "Web"]);
    // a first child cannot indent
    expect(indent(tree, byTitle(tree, "A.1").id)).toBe(tree);
    tree = outdent(tree, byTitle(tree, "B").id);
    expect(titles(tree)).toEqual(["A", "  A.1", "  A.2", "B", "  B.1", "Web"]);
    // the top level cannot outdent
    expect(outdent(tree, byTitle(tree, "A").id)).toBe(tree);
  });

  it("moves among siblings, removes a subtree and inserts after the selection", () => {
    let tree = toEditTree(SAMPLE);
    tree = moveBy(tree, byTitle(tree, "A.2").id, -1);
    expect(titles(tree).slice(0, 3)).toEqual(["A", "  A.2", "  A.1"]);
    expect(moveBy(tree, byTitle(tree, "A").id, -1)).toBe(tree);
    tree = removeNode(tree, byTitle(tree, "B").id);
    expect(titles(tree)).toEqual(["A", "  A.2", "  A.1", "Web"]);
    const node: EditNode = { id: newId(), title: "New", page: 5, open: true, children: [] };
    tree = insertAfter(tree, byTitle(tree, "A.2").id, node);
    expect(titles(tree)).toEqual(["A", "  A.2", "  New", "  A.1", "Web"]);
    tree = insertAfter(tree, null, { ...node, id: newId(), title: "Last" });
    expect(titles(tree).at(-1)).toBe("Last");
  });

  it("drag and drop: before / after / inside, never into its own subtree", () => {
    let tree = toEditTree(SAMPLE);
    const a = byTitle(tree, "A").id;
    tree = moveNode(tree, byTitle(tree, "Web").id, a, "before");
    expect(titles(tree)[0]).toBe("Web");
    tree = moveNode(tree, byTitle(tree, "B.1").id, byTitle(tree, "A.1").id, "inside");
    expect(titles(tree)).toEqual(["Web", "A", "  A.1", "    B.1", "  A.2", "B"]);
    expect(findNode(tree, byTitle(tree, "A.1").id)?.open).toBe(true);
    // A onto its own grandchild: refused
    expect(moveNode(tree, a, byTitle(tree, "B.1").id, "after")).toBe(tree);
    tree = moveNode(tree, byTitle(tree, "B").id, a, "after");
    expect(titles(tree).slice(-1)).toEqual(["B"]);
  });

  it("the operations never mutate their input, and sameOutline sees through ids", () => {
    const tree = toEditTree(SAMPLE);
    const snapshot = JSON.stringify(toOutline(tree));
    indent(tree, byTitle(tree, "B").id);
    removeNode(tree, byTitle(tree, "A").id);
    updateNode(tree, byTitle(tree, "A.1").id, { title: "renamed", dest: undefined });
    expect(JSON.stringify(toOutline(tree))).toBe(snapshot);
    expect(sameOutline(tree, toEditTree(SAMPLE))).toBe(true);
    const renamed = updateNode(tree, byTitle(tree, "A.1").id, { title: "renamed", dest: undefined });
    expect(sameOutline(renamed, tree)).toBe(false);
    expect(toOutline(renamed)[0].children[0]).toEqual({ title: "renamed", page: 1, children: [] });
  });

  it("collapsed nodes hide their descendants in the flat rows", () => {
    const tree = toEditTree(SAMPLE);
    const a = byTitle(tree, "A").id;
    expect(flattenEdit(tree, new Set([a])).map((r) => r.node.title)).toEqual(["A", "B", "B.1", "Web"]);
  });
});
