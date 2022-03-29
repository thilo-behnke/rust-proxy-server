# About

Simple http proxy written in rust, that accepts GET and CONNECT requests.

GET:
- Forward the request between client and host

CONNECT:
- Create a tcp tunnel between client and host
