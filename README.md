# TIDAL SDK

The TIDAL SDK consists of a set of software modules that defines, enables and simplifies usage of functionality provided
by TIDAL.

We aim to align functionality, naming, concepts and high level architecture across different implementations of the
TIDAL SDK (e.g. [TIDAL SDK for Web](https://github.com/tidal-music/tidal-sdk-web), [TIDAL SDK for Android](https://github.com/tidal-music/tidal-sdk-android) and [TIDAL SDK
for iOS](https://github.com/tidal-music/tidal-sdk-ios)) by writing engineering design docs for the SDK modules in
a [platform](https://en.wikipedia.org/wiki/Computing_platform)-agnostic way. Each design doc outlines the
responsibility, [API](https://en.wikipedia.org/wiki/API), requirements, functionality, intent and semantics of a module,
without enforcing implementation in a certain programming language or platform.

![TIDAL SDK](https://github.com/tidal-music/tidal-sdk/blob/main/media/readme-tidal-sdk-overview.png)

There are several distinct, but related goals we hope to achieve with the TIDAL SDK. Some of these goals aim at
improving the current way we build apps, while others aim at helping TIDAL through its ongoing transition from a "single
product, closed technology" company, to a "multi product, open technology" company. The main goals for the TIDAL SDK are
presented in this list.

* Align high-level architecture and terminology across different implementations of the same TIDAL app
    * By aligning modules across our SDK implementations, we will gradually create a common high-level architecture and
      terminology across implementations of the same TIDAL app.
    * We believe this will enable a higher degree of collaboration between the people working on different platforms (
      e.g. iOS, Android and Web).
* Enable faster prototyping and design of new products internally
    * By supplying a set of modules that can be [reused](https://en.wikipedia.org/wiki/Reusability) across different
      apps, the TIDAL SDK aims to reduce the number of times we need to reinvent the wheel when prototyping or building
      new products.
* Enable TIDAL Developer Platform
    * The implementations of the TIDAL SDK shall be made publicly available, playing a crucial part in
      the [TIDAL Developer Platform](https://developer.tidal.com) initiative as a complement to, and extension of,
      the TIDAL API.
* Define domains and ownership
    * The proposed mindset will result in each module also defining the responsibilities of a service, a service that
      potentially also includes backend functionality. As such, each module defines a clear domain that can also be
      allocated a clear ownership.
* Create a more predictable traffic pattern towards the TIDAL backend
    * Given that most apps (internal and external) will use the TIDAL SDK to gain access to the services supplied by
      TIDAL, we can control access patterns, retry logic etc. to the TIDAL backend, creating, and optimizing for, a
      certain traffic pattern.

## Designing a module

The following bullet list outlines some practices that we strive for when designing a module, helping answer the
questions "_What should be formalized as a module?_" and "_How should I design the module?_".

* **Relevance** - Modules should have functionality and responsibility that makes them relevant from a reusability,
  systems, product or SDK perspective. This formulation is deliberately made vague since it is extremely difficult
  to give a general answer to what is relevant and what is not relevant. We will need to make a case by case evaluation
  of each proposed module and learn as we go.
* **Product and service mindset** - The question “Does the module encapsulate a responsibility and functionality that
  makes sense to treat as a product/service in itself?” should be asked in the process of identifying modules. Even
  seemingly simple modules can make sense. What is most relevant when defining a module is whether it defines a clear
  and relevant area of responsibility and functionality or not, not how complex the implementation would be.
* **Encapsulation of backend communication** - When it comes to communication with backend, there are several good
  arguments for having backend communication encapsulated by a module owned by people that have a good understanding of
  the communication. Things like error handling, retry logic and performance improvements typically require a good
  understanding of the entire system (frontend/backend) to get right and optimal.
* **Low [coupling](https://en.wikipedia.org/wiki/Coupling_(computer_programming))** - Modules should be as independent
  as possible from one another, so that changes to one module have zero or
  minimal impact on other modules. Modules shall not have knowledge of the inner workings of other modules. Strive for a
  [functional design](https://en.wikipedia.org/wiki/Functional_design) mindset when designing modules.
* **High [cohesion](https://en.wikipedia.org/wiki/Cohesion_(computer_science))** - Modules should comprise a collection
  of code that acts as a system. They should have clearly defined responsibilities and stay within boundaries of certain
  domain knowledge.
* **Expose as little as possible** - The API of a module should be minimal and expose only the essentials. The module
  should efficiently fulfill its defined responsibility, but in a way that is as easy and fail-safe for the user of the
  module as possible. Always think about the module API from the user of the module’s point of view.
* **[Interface Segregation Principle](https://en.wikipedia.org/wiki/Interface_segregation_principle)** - If the module
  exposes functionality where different parts are relevant only for
  different users of the module, specify segregated interfaces for these different parts using roles.
* **No exotic dependencies** - Modules shall not have any “exotic” dependencies, for example to UI components or special
  [frameworks](https://en.wikipedia.org/wiki/Software_framework). We want modules to solve the business/model parts tied
  to the respective domain, isolating that logic in the module and making the module usable in as many settings as
  possible. If desired, a wrapper module can be supplied for a specific framework, to make it easier to use. For example
  the generic Web implementation of the Auth module could be wrapped by several other modules, e.g. Auth-Nuxt and
  Auth-React.

Once decided that a new module is to be designed and added to the SDK, create a new design doc using
the [template](TEMPLATE.md), and start writing.

## Concepts

Here are a few concepts that we try to align throughout the design docs.

### Error

Errors are used for encapsulating information about a failure that occurred in the module. Errors can be _raised_ by the
module to inform surrounding code that the module could not successfully complete a requested operation, and has given
up trying to do so. Code outside the module can use errors to understand what caused the failure and respond
accordingly.

* The name of an error shall be suffixed `Error` and defines the _error type_. The error type is used by code
  outside the module to understand what type of error has occurred and to decide upon appropriate actions.
* All errors have a default property named _errorCode_, used for defining the so-called error code. The error code,
  which is a `String` following the regexp `[0-9a-z]{1,5}`, is only intended to be used for debugging purposes in the
  case the app decides to present the error to the end user.
* By defining each error to have an error type and an error code, we are able to align end-user error messaging across
  implementations and improve debugging of error messages within our apps. The error type will make sure that the same
  error is presented to the end user in the same way, regardless of which implementation of an app they are using. The
  error code will give engineers, QA, CS and potentially also the end user more detailed information of what actually
  failed.
* It is always the responsibility of the app itself to decide whether an error should be presented to the end user, not
  the module’s. However, should the app decide to present an error to the end user, the error type and error code are
  intended to be used accordingly:
    * The error type shall be used by the app to look up a text string to present to the end user. For example, the
      error type `NoConnectionError` could map to a text saying "_There seems to be something wrong with your
      connection, please try later._".
    * The error code shall be appended to the text looked up from the error type. For example, building on the example
      above, an error with error type `NoConnectionError` and error code `A08H` shall be presented to the end user as: "
      _There seems to be something wrong with your connection, please try later. (A08H)_".

### Message

Messages are used for encapsulating information about an "interesting change" that occurred in the module. Messages can
be _fired_ by the module to inform surrounding code of "interesting changes" (
compare [event](https://en.wikipedia.org/wiki/Event_(computing))). Code outside the module can use messages to take
certain actions.

* The name of a message shall be suffixed `Message` and defines the _message type_. The message type is used by
  code outside the module to understand what type of interesting change that has occurred, and how to interpret any
  additional information the message may contain.

### Bus

A bus enables an [event-driven architecture](https://en.wikipedia.org/wiki/Event-driven_architecture) by allowing parts
of the system outside the module to subscribe/listen to messages or errors sent on the bus (
compare [event](https://en.wikipedia.org/wiki/Event_(computing))).

A common pattern for modules performing background operations is that the module wants to send messages to other parts
of the system, asynchronously informing them of an interesting event or a failure. Buses supply a shorthand way of
describing the intent of using this pattern.

* Messages and Errors can be sent on buses.
* A bus implementation can be either synchronous (
  e.g. [observer pattern](https://en.wikipedia.org/wiki/Observer_pattern)) or asynchronous (
  e.g. [pub/sub](https://en.wikipedia.org/wiki/Publish%E2%80%93subscribe_pattern)
  or [message queue](https://en.wikipedia.org/wiki/Message_queue) pattern).
