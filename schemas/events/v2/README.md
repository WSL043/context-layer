# Event schema v2 staging

This directory stages compatibility fixtures for the Personal Context v2 event envelope.

The v2 contract separates envelope evolution from payload evolution and permits raw retention of event types that the current projection layer does not understand yet. Built-in typed payloads remain supported, but unknown future event payloads must not be dropped solely because projection code is older.

Concrete v2 fixtures will be checked in together with the contract migration once the compatibility path is implemented and tested.
