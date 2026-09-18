package dev.openforge

import com.intellij.openapi.components.Service
import java.net.URI
import java.net.http.HttpClient
import java.net.http.HttpRequest
import java.net.http.HttpResponse
import java.util.concurrent.atomic.AtomicLong

@Service
class OpenForgeService {
    private val http = HttpClient.newBuilder().build()
    private val ids = AtomicLong()

    @Volatile
    var daemonUrl: String = "http://127.0.0.1:8765"

    fun rpc(method: String, paramsJson: String = "{}"): String {
        val requestId = ids.incrementAndGet()
        val body =
            "{"jsonrpc":"2.0","id":" + requestId +
                ","method":"" + escape(method) +
                "","params":" + paramsJson + "}"

        val request =
            HttpRequest.newBuilder(URI.create(daemonUrl + "/v1/rpc"))
                .header("content-type", "application/json")
                .POST(HttpRequest.BodyPublishers.ofString(body))
                .build()

        val response = http.send(request, HttpResponse.BodyHandlers.ofString())
        if (response.statusCode() !in 200..299) {
            throw IllegalStateException(
                "OpenForge daemon returned HTTP " +
                    response.statusCode() + ": " + response.body()
            )
        }
        return response.body()
    }

    private fun escape(value: String): String =
        value.replace("\\", "\\\\").replace(""", "\\"")
}
