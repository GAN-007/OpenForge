import { useMemo, useState } from "react";
import type { FileRecord } from "@openforge/sdk";

type TreeNode = {
  name: string;
  path: string;
  directory: boolean;
  children: TreeNode[];
};

function buildTree(files: FileRecord[]): TreeNode[] {
  const root: TreeNode = {
    name: "",
    path: "",
    directory: true,
    children: [],
  };

  for (const file of files) {
    const parts = file.path.replaceAll("\\", "/").split("/").filter(Boolean);
    let current = root;
    let path = "";
    for (let index = 0; index < parts.length; index += 1) {
      const name = parts[index]!;
      path = path ? path + "/" + name : name;
      const directory = index < parts.length - 1;
      let next = current.children.find(
        (child) => child.name === name && child.directory === directory,
      );
      if (!next) {
        next = { name, path, directory, children: [] };
        current.children.push(next);
      }
      current = next;
    }
  }

  const sort = (nodes: TreeNode[]) => {
    nodes.sort((left, right) => {
      if (left.directory !== right.directory) return left.directory ? -1 : 1;
      return left.name.localeCompare(right.name);
    });
    for (const node of nodes) sort(node.children);
  };
  sort(root.children);
  return root.children;
}

function NodeView(props: {
  node: TreeNode;
  depth: number;
  activePath: string | undefined;
  dirtyPaths: Set<string>;
  onOpen: (path: string) => void;
}) {
  const { node, depth, activePath, dirtyPaths, onOpen } = props;
  const [expanded, setExpanded] = useState(depth < 1);

  if (node.directory) {
    return (
      <>
        <button
          className="tree-row tree-directory"
          style={{ paddingLeft: 10 + depth * 14 }}
          onClick={() => setExpanded((value) => !value)}
          title={node.path}
        >
          <span className="tree-twist">{expanded ? "▾" : "▸"}</span>
          <span>{node.name}</span>
        </button>
        {expanded &&
          node.children.map((child) => (
            <NodeView
              key={child.path}
              node={child}
              depth={depth + 1}
              activePath={activePath}
              dirtyPaths={dirtyPaths}
              onOpen={onOpen}
            />
          ))}
      </>
    );
  }

  return (
    <button
      className={"tree-row tree-file" + (activePath === node.path ? " active" : "")}
      style={{ paddingLeft: 28 + depth * 14 }}
      onClick={() => onOpen(node.path)}
      title={node.path}
    >
      <span className="tree-file-name">{node.name}</span>
      {dirtyPaths.has(node.path) && <span className="tree-dirty">●</span>}
    </button>
  );
}

export function FileTree(props: {
  files: FileRecord[];
  activePath: string | undefined;
  dirtyPaths: Set<string>;
  filter: string;
  onOpen: (path: string) => void;
}) {
  const { files, activePath, dirtyPaths, filter, onOpen } = props;
  const filtered = useMemo(() => {
    const query = filter.trim().toLowerCase();
    if (!query) return files;
    return files.filter((file) => file.path.toLowerCase().includes(query));
  }, [files, filter]);
  const tree = useMemo(() => buildTree(filtered), [filtered]);

  if (!tree.length) {
    return <div className="sidebar-empty">No matching files.</div>;
  }

  return (
    <div className="file-tree">
      {tree.map((node) => (
        <NodeView
          key={node.path}
          node={node}
          depth={0}
          activePath={activePath}
          dirtyPaths={dirtyPaths}
          onOpen={onOpen}
        />
      ))}
    </div>
  );
}
