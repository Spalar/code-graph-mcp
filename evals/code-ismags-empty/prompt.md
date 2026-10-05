---
description: "Control: a local bug fix whose file and traceback are given (upstream networkx c94928e, reverted). Structure knowledge should not matter; compare turns and cost to see the plugin's overhead."
max_turns: 100
timeout_seconds: 1800
allowed_tools: [Read, Glob, Grep, Bash, Edit, Write, Agent]
tags: [coding]
---

In this networkx checkout, `ISMAGS.largest_common_subgraph()` (networkx/algorithms/isomorphism/ismags.py) crashes when no node of the subgraph can match any node of the graph:

```python
import networkx as nx
from networkx.algorithms import isomorphism as iso

g = nx.path_graph(5)
h = nx.path_graph(5)
for n in h:
    h.nodes[n]["color"] = "blue"
nm = nx.isomorphism.categorical_node_match("color", None)
list(iso.ISMAGS(g, h, node_match=nm).largest_common_subgraph())
# ValueError: min() iterable argument is empty
```

It should yield no mappings in that case (the same when the only obstacle is that every graph node has a self-loop the subgraph lacks), and it should not crash when either graph is empty. Please fix it.
