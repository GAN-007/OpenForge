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
    private val methodPattern = Regex("[A-Za-z0-9_./-]+")

    @Volatile
    var daemonUrl: String = "http://127.0.0.1:8765"

    @Volatile
    var apiToken: String? = null

    fun rpc(method: String, paramsJson: String = "{}"): String {
        require(methodPattern.matches(method)) { "invalid RPC method" }
        require(paramsJson.trim().startsWith("{")) { "RPC params must be a JSON object" }

        val requestId = ids.incrementAndGet()
        val body =
            """{"jsonrpc":"2.0","id":$requestId,"method":"$method","params":$paramsJson}"""

        val builder =
            HttpRequest.newBuilder(URI.create(daemonUrl.trimEnd('/') + "/v1/rpc"))
                .header("content-type", "application/json")
        apiToken
            ?.takeIf { it.isNotBlank() }
            ?.let { builder.header("authorization", "Bearer $it") }

        val request =
            builder
                .POST(HttpRequest.BodyPublishers.ofString(body))
                .build()

        val response = http.send(request, HttpResponse.BodyHandlers.ofString())
        if (response.statusCode() !in 200..299) {
            throw IllegalStateException(
                "OpenForge daemon returned HTTP " +
                    response.statusCode() + ": " + response.body(),
            )
        }
        return response.body()
    }
}
