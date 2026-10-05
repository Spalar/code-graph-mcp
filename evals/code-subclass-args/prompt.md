---
description: "Bug fix with parallel implementations: graph subclasses cannot take extra constructor arguments; the same `__new__` signature sits in several graph classes (upstream networkx 75bdd73, reverted)."
max_turns: 100
timeout_seconds: 1800
allowed_tools: [Read, Glob, Grep, Bash, Edit, Write, Agent]
tags: [coding]
---

In this networkx checkout, subclassing a graph class and adding a constructor argument breaks:

```python
import networkx as nx

class MyGraph(nx.Graph):
    def __init__(self, incoming_graph_data=None, extra_arg=None, **attr):
        super().__init__(incoming_graph_data, **attr)
        self.extra_arg = extra_arg

MyGraph(extra_arg="x")   # TypeError
```

Subclasses of the graph classes should be able to take extra positional or keyword arguments. Please fix it.
