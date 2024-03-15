# Discv5 with Discv4 support

A Discv5 node and a Discv4 node run alongside each other on different ports. The node
records of both are signed with the same key. Hence both nodes share the same node id.
Discv4 makes sure that it isn't connected to any of the same peers as Discv5. In this
sense, Discv5 is favoured, and Discv4 is available as downgrade.