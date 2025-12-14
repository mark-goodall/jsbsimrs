# A rust JSBSim interface

This is a fairly simple barebones interface for JSBSim to allow for connecting a rust app to the JSBSim flight dynamics.
JSBSim is run as a separate process, following how other software integrate with JSBSim like ardupilot and px4.

Included is a minimal JSBSim root directory with a modified Concorde model to expose the interface on port 5556, which is used for testing.

## JSBSim Gotchas

* Setting the simulator rate (in hz) too low resulted in instabilities (SIGFPE).
* When the simulator crashes, sometimes the port is kept in a weird state, which prevents further tests passing.
