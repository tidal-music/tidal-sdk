# Event Producer

The TIDAL Event Platform (TEP) is the system responsible for defining, producing, transporting, routing and consuming
events in TIDAL.

An event represents an interesting piece of information that a TIDAL app wants to send to the TIDAL backend for further
analysis and/or processing. The following structure defines an event:

```java
class Event {

    // Unique ID for each event instance. E.g. UUID or NanoID. (TL)
    String id;

    // Defines a certain type of event. E.g. "play-log". (TL/AL)
    String name;

    // Headers containing metadata attached to the payload. (AL)
    Map<String, String> headers;

    // Contains business information the app wants to send. (AL)
    String payload;
}
```

The TEP is designed using a layered architecture, separating the system into
two [abstraction layers](https://en.wikipedia.org/wiki/Abstraction_layer) called the _application layer_ (AL) and the
_transportation layer_ (TL). A layered architecture allows us to clearly separate the _what_ (AL) from the _how_ (TL)
when it comes to event transportation, resulting in a
clear [separation of concerns](https://en.wikipedia.org/wiki/Separation_of_concerns) that allows us to improve and
modify each layer separately.

The TL is agnostic to the implementation of the AL, so multiple AL implementations can use the same TL. There can be one
or several _AL Producers_ with corresponding _AL Consumers_ using the same _TL Producer/Consumer_, as shown in the
figure below.

![TIDAL Event Platform](https://github.com/tidal-music/tidal-sdk/blob/main/media/event-producer-tidal-event-platform.png)

The responsibility of the AL is to make sure that the content, format and structure of event headers and payload are
correct and fulfills a certain business purpose. All requirements related to this functionality are attached to the AL.

The responsibility of the TL is to make sure that an event gets transported from the TL Producer to the TL Consumer as
fast, secure and reliable as possible. The TL is also responsible for routing events to the correct AL Consumer once
received by the TL Consumer. All requirements related to this functionality are attached to the TL.

The _EventProducer_ module represents an implementation of the TEP TL Producer.

# Terminology

* _Event_ - An interesting piece of information that a TIDAL app wants to send to the TIDAL backend for further analysis
  and/or processing. The information contained in events is typically used for business analytics, monitoring purposes
  or similar.
* _TIDAL Event Platform (TEP)_ - The system responsible for defining, producing, transporting, routing and consuming
  events in TIDAL.
* _Application layer (AL)_ - The part of the TIDAL event platform that is responsible for defining the format, structure
  and content of the event payload.
* _Transportation layer (TL)_ - The part of the TIDAL event platform that is responsible for transporting events from
  the app to the backend.
* _Outage_ - Occurs when the EventProducer identifies that it cannot transport events to the TL Consumer as expected.

# API

## EventSender

Exposes functionality for sending events and monitoring the status of the TEP TL.

### init

```java
/**
 * In order to simplify the implementation while still maintaining correct function, high performance and reduced load on backend, a design decision has been made that the EventSender shall only ever exist in one instance.
 *
 * @param tlConsumerUri URI identifying the TL Consumer ingest endpoint.
 * @param credentialsProvider A credentials provider, used by the EventProducer to get access token.
 * @param maxDiskUsageBytes The maximum amount of disk the EventProducer is allowed to use for temporarily storing events before they are sent to TL Consumer.
 * @param blockedConsentCategories Used to initialize the blockedConsentCategories property
 * @param appName added as a header for each sent event
 * @param appVersion added as a header for each sent event
 */
init(URI tlConsumerUri,
     Auth.CredentialsProvider credentialsProvider,
     Int maxDiskUsageBytes,
     Set<ConsentCategory> blockedConsentCategories,
     String appName,
     String appVersion);
```

### sendEvent

```java
/**
 * Sends an event to the TL Consumer.
 *
 * Async and thread safe. A call to this function can be assumed to take a negligible amount of time, 
 * and can be called from any context.
 *
 * @param eventName The name of the event
 * @param consentCategory The consent category the event belongs to
 * @param payload The payload of the event, i.e. the actual business data being sent.
 * @param headers Optional headers the app want to send together with the event.
 */
Void sendEvent(
        String eventName,
        ConsentCategory consentCategory,
        String payload,
        Map<String, String>? headers=null);
```

### isOutage

```java
/**
 * This read-only property indicates whether there is an outage.
 * Each time this property toggles value, a corresponding OutageStartError or OutageEndMessage is sent on the bus.
 * See "Outage notification" and "Design" sections for more information.
 * Persisted by the EventProducer module.
 */
Bool isOutage;
```

### bus

```java
/**
 * The bus used by the EventSender for all "asynchronous" communication.
 */
Bus<OutageStartError, OutageEndMessage> bus;
```

## Config

The Config role exposes functionality for configuring the EventProducer.

### blockedConsentCategories

```java
/**
 * Defines a set of consent categories for which the EventProducer shall drop events.
 * Initialized by the corresponding parameter in the init function.
 */
Set<ConsentCategory> blockedConsentCategories;
```

## Types

### ConsentCategory

```java
/**
 * Consent categories allow the end user to control how TIDAL can use information gathered via events. 
 * Each event belongs to one consent category, and the user can opt out a consent category.
 */
enum ConsentCategory {
    // The event is considered strictly necessary. End users cannot opt out of strictly necessary events.
    NECESSARY,

    // The event is used e.g. for advertisement. End users can opt out of targeting events. Also called advertising.
    TARGETING,

    // The event is used e.g. for tracking the performance and usage of the app. End users can opt out of performance events. Also called analytics.
    PERFORMANCE
}
```

### OutageStartError

```java
/**
 * An error indicating that an outage has started.
 * See "Outage notification" and "Design" sections for more information.
 */
class OutageStartError {
}
```

### OutageEndMessage

```java
/**
 * A message indicating that an outage has ended.
 * See "Outage notification" and "Design" sections for more information.
 */
class OutageEndMessage {
}
```

# Functionality and Requirements

## Fault tolerance

The EventProducer will not guarantee delivery of events, but it shall monitor and report dropped events, guaranteeing to
the extent possible, that either an event is delivered, or it is reported as dropped.

The EventProducer is designed to be extremely easy to use. The `sendEvent` function shall act as a “fire-and-forget”
solution for sending events, meaning that the end user will not know for sure whether the event will get delivered to
the TEP ingest service or not. The EventProducer shall, however, supply functionality for monitoring the health of the
TEP TL. See Outage notification and Monitoring sections for further information.

## Performance

The `sendEvent` function shall take a negligible amount of time to execute before it returns.

This requirement aims at making the EventProducer as easy to use as possible. If the user of the EventProducer can
assume that sendEvent returns within a negligible amount of time, the user can make calls to `sendEvent` in whichever
context or part of the program that is most convenient, without having to think of performance issues.

Even though a somewhat vague requirement, it is important to describe the intended functionality of the module.

## Event validation

* Any event exceeding a total event size (all fields included) of 20 KiB (20480 B), when encoded according to
  this [W3C specification](https://www.w3.org/TR/REC-xml/#charsets), shall be dropped. The number 20KiB is chosen as: 1.
  We believe it is enough for all events we want to send, 2. It allows us to send full AWS SQS batches (max batch size
  is 10 messages / 256 KiB), 3. It leaves some headroom for enriching each event, while still fulfilling point 2.
* Any event containing any character not included in this [W3C specification](https://www.w3.org/TR/REC-xml/#charsets)
  shall be dropped.

Many messaging/stream solutions have an upper limit of the size of the messages it transports. For example, Amazon
Kinesis Data Streams documentation states that: “The maximum size of the data payload of a record before base64-encoding
is up to 1 MB”, the default maximum message size for a kafka message is also 1MB and the maximum message (and batch)
size for AWS SQS is 256 KiB. The TEP TL Consumer already relies on both AWS SQS and Kinesis for delivering events to AL
Consumers, so the smallest limit set by these systems must be enforced. Also, enforcing a maximum limit gives us the
freedom to change the underlying implementation of the TEP TL layer in the future to, without having to worry about the
max size constraint being violated.

In addition to putting limitations on event size, some messaging solutions (e.g. AWS SQS) put constraints on the charset
used for the message payload. Hence the requirement of the supported charset.

## Consent filtering

In order to comply with GDPR and other privacy regulations, EventProducer must implement a filtering mechanism to
exclude events for which users did not provide consent.

Each event must be assigned a consent category, which is listed in the `ConsentCategory` enum class. Using this field,
EventProducer should ensure that events falling under categories for which users did not provide consent are not
transmitted.

To ensure that unauthorized events are not sent, filtering must be implemented prior to adding events to the queue.

Finally, it should be ensured that consent categories for which users have consented can be changed on the fly while
using the app.

## Header enrichment

The EventProducer shall enrich an event with default headers. Even though headers are strictly speaking part of the AL,
the EventProducer supplies this functionality in order not to replicate it across all AL Producers.

The following default headers shall be added to each event by the EventProducer:

| Key                        | Value                                                                                                 |
|----------------------------|-------------------------------------------------------------------------------------------------------|
| `authorization`            | OAuth JWT token. Only for logged in users.                                                            |
| `client-id`                | Client id.                                                                                            |
| `consent-category`         | The event’s consent category. E.g. "NECESSARY"                                                        |
| `requested-sent-timestamp` | The number of milliseconds since Unix epoch when the call to sendEvent was made. E.g. "1686293314385" |
| `device-vendor`            | Name of device vendor. E.g. "Samsung", "Apple"                                                        |
| `device-model`             | Name of device model. E.g. "Galaxy S8", "iPhone 4"                                                    |
| `os-name`                  | Name of operating system. E.g. "Android", "iOS", "Windows"                                            |
| `os-version`               | Version of operating system. E.g. "1.1"                                                               |
| `browser-name`             | Name of browser (webapps only). E.g. "Chrome"                                                         |
| `browser-version`          | Version of browser (webapps only). E.g. "1.1"                                                         |
| `app-version`              | Version of TIDAL app. E.g. "1.1"                                                                      |

Any custom headers supplied by the caller of `sendEvent` have precedence over default headers. I.e. if the caller
of `sendEvent` supplies a header with the same key as any of these default headers, no enrichment shall take place for
that header, and the supplied header shall be added to the event.

Custom headers shall always be added to the resulting event, but if no valid value exists for a default header, the
entire default header shall be omitted. For example, if a user is logged out, the entire default authorization header
shall be omitted.

## Event ordering

The EventProducer shall make a best effort to preserve the order of events, but might deliver duplicate events, or out
of order events depending on different failures that may occur.

## Outage notification

The EventProducer shall continuously monitor the health of the TEP TL and communicate any unexpected behavior (events
not reaching the TEP TL consumer as expected) to the app.

Outage notification gives the app the opportunity to take action if event sending doesn't function properly. An example
could be that we don’t want to allow playback of media products unless event sending works as expected. By leaving the
decision of whether there is an outage up to the EventProducer, we aim at simplifying such functionality for the
app.

As of now, an outage is simply defined as taking place when the EventProducer unexpectedly drops events. However, the
outage definition could be expanded in future revisions of the EventProducer to also incorporate other cases, for
example if we detect ad-blockers blocking event sending.

The module supports the following ways of tracking whether there is an outage:

* `isOutage` property. This property can be queried to see if there is an outage or not.
* `OutageStart` error. Sent whenever the `isOutage` property toggles from `False` to `True`.
* `OutageEnd` message. Sent whenever the `isOutage` property toggles from `True` to `False`.

The intended behavior of the outage notification is explained more in detail in the Design section below.

## Monitoring

The EventProducer shall continuously report statistics about the TEP TL that can be analyzed offline, allowing us to
understand and improve the TEP TL.g

The EventProducer shall gather monitoring information in an internal structure, and send that information every 60
seconds in the form of a `tep-tl-monitoring` event. It means that it shall be sent as a normal event where
the `MonitoringInformation` (serialized as JSON) shall be the payload, and `tep-tl-monitoring` shall be the event name.

The EventProducer shall store monitoring information in a structure separate to the one holding “normal” events, so that
monitoring information can be gathered even if the logic handling normal events fails. The structure holding monitoring
information shall be persisted, so that the risk of losing monitoring information is minimized.

The following information shall be gathered and sent in the payload of the `tep-tl-monitoring` event.

```java
class MonitoringInformation {
    /**
     * Each map maps an event name to a counter.
     * The counter indicates how many events of a certain type that was dropped due to filtering, validation or storing issues respectively
     */
    Map<String, Int> consentFilteredEvents;
    Map<String, Int> validationFailedEvents;
    Map<String, Int> storingFailedEvents;
}
```

The monitoring event shall be enriched with default headers before being sent, but it shall not include any information
identifying a user, guaranteeing that the monitoring event can always be sent without violating any user consent. This
means that header enrichment for the monitoring event shall never contain the authorization header, only the client-id,
even if the user is logged in.

The consent category for the `tep-tl-monitoring` event shall be set to `NECESSARY` to guarantee that any potential
future filtering on backend does not filter out these events.

## Design

This section outlines a proposed high-level design that aims at being simple to implement and reason about, while at the
same time fulfilling the other requirements listed in this specification. The proposal here should be treated as a
design describing the intended behavior of the EventProducer, not a strict design description.

The design uses a Job queue pattern for sending events, where each event is considered a job. As such, the design
divides the functionality of sending an events into three major parts:

* _Submitter_ - This logic is triggered by the sendEvent function. The responsibility of the submitter logic is to
  perform basic processing of the event and storing it in the Queue, gathering information about anything that causes
  the event not to end up in the queue.
* _Queue_ - A data structure in persistent memory that contains events that are submitted, but not yet confirmed
  received by the TL Consumer.
* _Scheduler_ - This logic is executed in a background process/thread owned by the EventProducer. The responsibility of
  this logic is to read events from the Queue, send them to the TL Consumer, and remove events from the Queue once
  confirmed received by the TL Consumer.

In terms of fault tolerance, there are three key aspects of this design to note:

1. The Queue must be kept in persistent storage. This means that even if the app crashes, the device running the app
   runs out of battery or anything else happens that causes the app to terminate its execution, the events will not be
   lost. The next time the EventProducer gets instantiated, the Queue will contain all events it had when the app
   exited.
1. The Submitter logic (initiated by the sendEvent function) shall monitor and gather statistics about each event it
   fails to add to the Queue. This means that either the event gets safely stored in persistent memory, or it will be
   added to the monitoring information.
1. The Scheduler logic shall never remove an event from the Queue unless it has received confirmation from the TL
   Consumer that the event has been received. This means that events will be safely stored in the Queue until positive
   confirmation has been received from the TL Consumer that it has received the events.

### Submitter

The Submitter logic is triggered by the `sendEvent` function. In case the submitter logic takes a long time to execute,
the `sendEvent` function can immediately hand over to a separate thread and return as soon as possible, guaranteeing
that it blocks the calling thread for as short a duration as possible. The submitter logic is also responsible for
handling outage notifications, as described in the diagram below.

![Submitter](https://github.com/tidal-music/tidal-sdk/blob/main/media/event-producer-submitter.png)

Some comments to the diagram:

* Register dropped event refers to the functionality described in the Monitoring section above.

### Queue

The Queue is a data structure backed by persistent storage that holds all submitted events that are not yet removed by
the Scheduler. It can use a file, a database or some other solution that guarantees events are not lost in case of the
app exiting. The Queue shall respect the maxDiskUsageBytes configuration parameter, resulting in new events being
dropped if size is exceeded.

```java
/**
 * Example interface of the Queue
 */
class Queue {

    /**
     * Adds an event to the event queue.
     * If this operation returns succesfully, the event shall be stored in persistant storage.
     * @param event The event to add to the queue
     * @throws xyz Any error that can occur that causes the event not to be added to the queue, e.g. configured max disk size is exceeded, disk is full or similar.
     */
    Void addEvent(Event event) throws xyz;

    /**
     * Get all events currently in the queue.
     * @return A list of all events currently in the queue. It must be impossible to modify the content of the queue using the returned value, e.g. return copy or immutable.
     */
    List<Event> getEvents();

    /**
     * Removes events from queue
     * @param events Events to be removed from queue
     * @throws xyz Any error that can occur that causes the events not to be removed from the queue
     */
    Void removeEvents(List<Event> events) throws xyz;
}
```

### Scheduler

The Scheduler periodically gets events from the Queue, batches them according to the TL Consumer API, sends them to the
TL Consumer, and removes events confirmed received by the TL Consumer from the Queue.

The Scheduler applies a best-effort approach to sending events to the TL Consumer, meaning that it continuously tries to
send events while the app is executing, regardless of current internet conditions or if the app considers itself to be
online or not. As long as the app is executing, the Scheduler shall be running

The diagram below describes the intended logic of the Scheduler. It aims at being as simple as possible to implement,
but at the same time fulfilling the requirements for the EventProducer.

![Scheduler](https://github.com/tidal-music/tidal-sdk/blob/main/media/event-producer-scheduler.png)

Some comments to the diagram:

* DELAY = 30 seconds
* BATCH_SIZE = Max batch size supported by the TL Consumer which is 10.
* If any unexpected error occurs during the execution, the logic shall restart the flow by jumping directly to “Sleep
  for DELAY seconds”.

In addition to the logic outlined in this section, it is also recommended to add logic for sending the monitoring event
into the Scheduler.
