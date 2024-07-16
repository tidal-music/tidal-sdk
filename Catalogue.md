# Catalogue

The TIDAL Catalogue module is a wrapper around our [Catalogue API](https://developer.tidal.com/reference/web-api?spec=catalogue).

It aims to provide simplified access to the API for clients.

# API

## Catalogue

The `Catalogue` exposes TIDAL catalogue endpoints. It uses [Auth`s]() `CredentialsProvider` to authenticate backend calls.

```java
/**
* @param credentialsProvider A credentials provider, used by the Catalogue to get access tokens.
* @param catalogueBaseUrl Optional base URL to the catalogue API endpoint. Defaults to TIDAL production environment.
*/
init(AuthCredentialsProvider credentialsProvider, URL? catalogueBaseUrl="https://openapi.tidal.com/v2/"),

```
