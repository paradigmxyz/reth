# RETH network implementation

Supports both devp2p peer discovery version 4 and 5. The default is Discv4. Discv5 
is available as a feature, as well as Discv5 with version downgrade to v4. Behind
the scenes, Discv5 with version downgrade runs a Discv4 node too, which can connect
to peers that don't connect over Discv5.