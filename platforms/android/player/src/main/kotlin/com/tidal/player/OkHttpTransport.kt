package com.tidal.player

import java.io.IOException
import java.io.InterruptedIOException
import java.net.SocketException
import java.net.UnknownHostException
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException
import kotlinx.coroutines.suspendCancellableCoroutine
import okhttp3.Call
import okhttp3.Callback
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response
import uniffi.player.HttpMethod
import uniffi.player.HttpRequest
import uniffi.player.HttpResponse
import uniffi.player.HttpTransport
import uniffi.player.TransportException

class OkHttpTransport(private val client: OkHttpClient) : HttpTransport {
    override suspend fun execute(request: HttpRequest): HttpResponse {
        val call = client.newCall(request.toOkHttpRequest())
        return suspendCancellableCoroutine { continuation ->
            continuation.invokeOnCancellation { call.cancel() }
            call.enqueue(object : Callback {
                override fun onResponse(call: Call, response: Response) {
                    try {
                        val body = response.use { it.body!!.bytes() }
                        continuation.resume(HttpResponse(response.code.toUShort(), body))
                    } catch (exception: IOException) {
                        continuation.resumeWithException(transportError(exception))
                    }
                }

                override fun onFailure(call: Call, e: IOException) {
                    continuation.resumeWithException(transportError(e))
                }
            })
        }
    }
}

private fun HttpRequest.toOkHttpRequest(): Request {
    val builder = try {
        Request.Builder().url(url)
    } catch (exception: IllegalArgumentException) {
        throw TransportException.OtherException("Invalid URL: $url")
    }
    headers.forEach { (name, value) -> builder.header(name, value) }
    val body = body ?: ByteArray(0).takeIf { method == HttpMethod.POST || method == HttpMethod.PATCH }
    return builder.method(method.name, body?.toRequestBody()).build()
}

private fun transportError(exception: IOException): TransportException = when (exception) {
    is InterruptedIOException -> TransportException.TimeoutException(exception.toString())
    is UnknownHostException, is SocketException ->
        TransportException.NetworkException(exception.toString())
    else -> TransportException.OtherException(exception.toString())
}
