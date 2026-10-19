# bevy_city

A procedurally generated city using assets from [kenney](https://kenney.nl)

This project was inspired by <https://flecs-hub.github.io/traffic/etc/>.
Currently it doesn't do any traffic simulation but the goal is to have a similar simulation but using bevy_ecs.

RS should handle encoding of video and data. Uses Moq-mux for video and data bitstreams and moq-lite handles network.
Streaming of generated city, rendered in browser via MoQ. JS should be browser player. should use moq/watch and moq/net
Set up a MoQ relay and prod env for this  


Verify this with moq-cli and publish with test token
moq play
check for /health and /metrics


