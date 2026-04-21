# TODO

## Current

container
- harness.rs has container specific knowledge (see NGINX_HOST comment), ideally keep that out of main repo
 - might need to change it to 
 ```
 env: 
   "NGINX_HOST": "localhost"
```
which would be fine

## Next

split actors into separate crates, this will allow users to easily create their own actors