# TODO

## Current


## Next

- final review of examples
- does shell make sense now?
  - everything should happen in a docker container
  - would be nice to maybe shell into a docker container, though
  - need to update error handling
  - punt?
- how to think about async http call?
    - ex: demonstrate last write wins between two clients
        - alice and bob both do a POST with an await
        - something happens at the server?
        - alice and bob both resolve
        - database shows last request
