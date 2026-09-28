/**
 * 목차 편집 (P2) — the outline as an editable tree. Pure functions: every operation returns a new
 * tree and leaves its input alone, so the editor can compare against the original and undo a
 * gesture by keeping the previous value.
 *
 * Nodes carry a local `id` (the outline has no identity of its own — `set_outline` replaces the
 * whole tree); `toOutline` drops it again and writes `open` only where a node has children, which
 * is exactly what `get_outline` reads back.
 */
import type { OutlineDest, OutlineNode, PageIndex } from "../ipc/types";

export interface EditNode {
  id: number;
  title: string;
  page: PageIndex | null;
  dest?: OutlineDest;
  url?: string;
  open: boolean;
  children: EditNode[];
}

export interface FlatEditRow {
  node: EditNode;
  depth: number;
  parentId: number | null;
}

export type DropPosition = "before" | "after" | "inside";

let nextId = 0;

export function newId(): number {
  return ++nextId;
}

export function toEditTree(nodes: OutlineNode[]): EditNode[] {
  return nodes.map((n) => {
    const node: EditNode = {
      id: newId(),
      title: n.title,
      page: n.page,
      open: n.open ?? true,
      children: toEditTree(n.children ?? []),
    };
    if (n.dest) node.dest = { ...n.dest };
    if (n.url) node.url = n.url;
    return node;
  });
}

export function toOutline(nodes: EditNode[]): OutlineNode[] {
  return nodes.map((n) => {
    const out: OutlineNode = { title: n.title, page: n.url ? null : n.page, children: toOutline(n.children) };
    if (n.dest && out.page !== null) out.dest = { ...n.dest };
    if (n.url) out.url = n.url;
    if (n.children.length) out.open = n.open;
    return out;
  });
}

/** Depth-first rows; `collapsed` hides a node's descendants (the node itself stays). */
export function flattenEdit(nodes: EditNode[], collapsed: ReadonlySet<number> = new Set()): FlatEditRow[] {
  const out: FlatEditRow[] = [];
  const walk = (list: EditNode[], depth: number, parentId: number | null) => {
    for (const node of list) {
      out.push({ node, depth, parentId });
      if (node.children.length && !collapsed.has(node.id)) walk(node.children, depth + 1, node.id);
    }
  };
  walk(nodes, 0, null);
  return out;
}

function clone(nodes: EditNode[]): EditNode[] {
  return nodes.map((n) => ({ ...n, dest: n.dest ? { ...n.dest } : undefined, children: clone(n.children) }));
}

interface Place {
  siblings: EditNode[];
  index: number;
  parent: EditNode | null;
}

/** Where `id` sits in `nodes` (the arrays are the tree's own, so a clone may be mutated through it). */
function locate(nodes: EditNode[], id: number, parent: EditNode | null = null): Place | null {
  for (let i = 0; i < nodes.length; i++) {
    if (nodes[i].id === id) return { siblings: nodes, index: i, parent };
    const inner = locate(nodes[i].children, id, nodes[i]);
    if (inner) return inner;
  }
  return null;
}

export function findNode(nodes: EditNode[], id: number): EditNode | null {
  const place = locate(nodes, id);
  return place ? place.siblings[place.index] : null;
}

function contains(node: EditNode, id: number): boolean {
  return node.id === id || node.children.some((c) => contains(c, id));
}

export function updateNode(nodes: EditNode[], id: number, patch: Partial<Omit<EditNode, "id" | "children">>): EditNode[] {
  const tree = clone(nodes);
  const place = locate(tree, id);
  if (!place) return nodes;
  const node = place.siblings[place.index];
  Object.assign(node, patch);
  if ("dest" in patch && patch.dest === undefined) delete node.dest;
  if ("url" in patch && patch.url === undefined) delete node.url;
  return tree;
}

/** Removes `id` and its whole subtree. */
export function removeNode(nodes: EditNode[], id: number): EditNode[] {
  const tree = clone(nodes);
  const place = locate(tree, id);
  if (!place) return nodes;
  place.siblings.splice(place.index, 1);
  return tree;
}

/** `node` as the next sibling of `afterId`, or at the end of the top level when `afterId` is null. */
export function insertAfter(nodes: EditNode[], afterId: number | null, node: EditNode): EditNode[] {
  const tree = clone(nodes);
  const place = afterId === null ? null : locate(tree, afterId);
  if (!place) tree.push(node);
  else place.siblings.splice(place.index + 1, 0, node);
  return tree;
}

/** 들여쓰기: the node becomes the last child of its previous sibling (no-op for a first child). */
export function indent(nodes: EditNode[], id: number): EditNode[] {
  const tree = clone(nodes);
  const place = locate(tree, id);
  if (!place || place.index === 0) return nodes;
  const [node] = place.siblings.splice(place.index, 1);
  const host = place.siblings[place.index - 1];
  host.children.push(node);
  host.open = true;
  return tree;
}

/** 내어쓰기: the node moves out of its parent, right after it (no-op at the top level). */
export function outdent(nodes: EditNode[], id: number): EditNode[] {
  const tree = clone(nodes);
  const place = locate(tree, id);
  if (!place || !place.parent) return nodes;
  const parentPlace = locate(tree, place.parent.id);
  if (!parentPlace) return nodes;
  const [node] = place.siblings.splice(place.index, 1);
  parentPlace.siblings.splice(parentPlace.index + 1, 0, node);
  return tree;
}

/** One step up or down among its siblings. */
export function moveBy(nodes: EditNode[], id: number, delta: -1 | 1): EditNode[] {
  const tree = clone(nodes);
  const place = locate(tree, id);
  if (!place) return nodes;
  const to = place.index + delta;
  if (to < 0 || to >= place.siblings.length) return nodes;
  const [node] = place.siblings.splice(place.index, 1);
  place.siblings.splice(to, 0, node);
  return tree;
}

/** Drag and drop: `id` before / after / as the last child of `targetId`. Never into its own subtree. */
export function moveNode(nodes: EditNode[], id: number, targetId: number, position: DropPosition): EditNode[] {
  if (id === targetId) return nodes;
  const moving = findNode(nodes, id);
  if (!moving || contains(moving, targetId)) return nodes;
  const tree = clone(nodes);
  const from = locate(tree, id)!;
  const [node] = from.siblings.splice(from.index, 1);
  const target = locate(tree, targetId);
  if (!target) return nodes;
  if (position === "inside") {
    const host = target.siblings[target.index];
    host.children.push(node);
    host.open = true;
  } else {
    target.siblings.splice(target.index + (position === "after" ? 1 : 0), 0, node);
  }
  return tree;
}

/** The two trees would write the same outline. */
export function sameOutline(a: EditNode[], b: EditNode[]): boolean {
  return JSON.stringify(toOutline(a)) === JSON.stringify(toOutline(b));
}

/** Counts every node of the tree. */
export function countNodes(nodes: EditNode[]): number {
  return nodes.reduce((n, node) => n + 1 + countNodes(node.children), 0);
}
