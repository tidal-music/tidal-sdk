# Common

The Common module holds types and functionality that is shared across different modules, like errors, messages,
interfaces and more. Content of this module will typically differ across implementations.

# API

## TidalError

```java
/**
 * Base error class all SDK modules' errors should extend.
 */
class TidalError {
    String errorCode; // Must match the regexp: [0-9a-z]{1,5} 
}
```

## NetworkError

```java
/**
 * An error to be raised for unexpected errors. Can be used as a "catch all" error.
 */
class NetworkError {
}
```

## RetryableError

```java
/**
 * This is a pretty "rough" and "unprecise" error that indicates to the user of the module that the operation failed, but it might succeed if the user retries it. It is intended to be raised in situations where the user of the module cannot do that much even if they got more precise information, for example server errors. However, the error code should supply relevant information for debugging.
 */
class RetryableError {
}
```

## UnexpectedError
```java
/**
 * An error to be raised for unexpected errors. Can be used as a "catch all" error.
 */
class NetworkError {
}
```

## IllegalConfigurationError
```java
/**
 * Raised whenever an operation failed due to an incorrect configuration.
 */
class IllegalConfigurationError {
}
```

## IllegalArgumentError
```java
/**
 * Raised whenever an illegal argument was passed to a function.
 */
class IllegalArgumentError {
}
```