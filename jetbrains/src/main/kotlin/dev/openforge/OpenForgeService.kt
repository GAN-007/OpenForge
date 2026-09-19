package dev.openforge

import com.intellij.credentialStore.CredentialAttributes
import com.intellij.credentialStore.generateServiceName
import com.intellij.ide.passwordSafe.PasswordSafe
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
    private val credentials =
        CredentialAttributes(generateServiceName("OpenForge", "daemon-token"))

    @Volatile
    var daemonUrl: String = "http://127.0.0.1:8765"

    fun apiToken(): String? = PasswordSafe.instance.getPassword(credentials)

    fun setApiToken(value: String?) {
        PasswordSafe.instance.setPassword(
            credentials,
            value?.trim()?.takeIf { it.isNotEmpty() },
        )
    }

    fun rpc(method: String, paramsJson: String = "{}"): String {
        require(methodPattern.matches(method)) { "invalid RPC method" }
        val params = paramsJson.trim()
        require(params.startsWith("{") && params.endsWith("}")) {
            "RPC params must be a JSON object"
        }

        val requestId = ids.incrementAndGet()
        val body =
            "{\"jsonrpc\":\"2.0\",\"id\":" +
                requestId +
                ",\"method\":\"" +
                method +
                "\",\"params\":" +
                params +
                "}"

        val builder =
            HttpRequest.newBuilder(URI.create(daemonUrl.trimEnd('/') + "/v1/rpc"))
                .header("content-type", "application/json")
        apiToken()
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
                    response.statusCode() +
                    ": " +
                    response.body(),
            )
        }
        val payload = response.body()
        if (payload.contains("\"error\":") && !payload.contains("\"error\":null")) {
            throw IllegalStateException("OpenForge RPC failed: $payload")
        }
        return payload
    }

    fun initialize(): String = rpc("initialize")

    fun repositoryIndex(repo: String): String =
        rpc("repository/index", "{\"repo\":" + quote(repo) + "}")

    fun search(repo: String, query: String, limit: Int = 50): String =
        rpc(
            "search/query",
            "{\"repo\":" +
                quote(repo) +
                ",\"query\":" +
                quote(query) +
                ",\"limit\":" +
                limit.coerceIn(1, 250) +
                "}",
        )

    fun createRun(
        repo: String,
        objective: String,
        autonomy: String,
        budgetUsd: Double,
    ): String =
        rpc(
            "run/create",
            "{\"repo\":" +
                quote(repo) +
                ",\"objective\":" +
                quote(objective) +
                ",\"autonomy\":" +
                quote(autonomy) +
                ",\"budget_usd\":" +
                budgetUsd +
                "}",
        )

    fun planRun(repo: String, runId: String): String =
        rpc(
            "run/plan",
            "{\"repo\":" + quote(repo) + ",\"run_id\":" + quote(runId) + "}",
        )

    fun executeRun(repo: String, runId: String, docker: Boolean): String =
        rpc(
            "run/execute",
            "{\"repo\":" +
                quote(repo) +
                ",\"run_id\":" +
                quote(runId) +
                ",\"docker\":" +
                docker +
                "}",
        )

    fun tasks(runId: String): String =
        rpc("task/list", "{\"run_id\":" + quote(runId) + "}")

    fun verifyAudit(): String = rpc("event/verify")

    fun agents(): String = rpc("agent/list")

    fun queueInstruction(threadId: String, instruction: String): String =
        rpc(
            "thread/instruction-queue",
            "{\"thread_id\":" +
                quote(threadId) +
                ",\"content\":" +
                quote(instruction) +
                "}",
        )

    companion object {
        fun quote(value: String): String {
            val output = StringBuilder(value.length + 2)
            output.append('"')
            for (character in value) {
                when (character) {
                    '\\' -> output.append("\\\\")
                    '"' -> output.append("\\\"")
                    '\b' -> output.append("\\b")
                    '\u000C' -> output.append("\\f")
                    '\n' -> output.append("\\n")
                    '\r' -> output.append("\\r")
                    '\t' -> output.append("\\t")
                    else ->
                        if (character.code < 0x20) {
                            output.append("\\u")
                            output.append(character.code.toString(16).padStart(4, '0'))
                        } else {
                            output.append(character)
                        }
                }
            }
            output.append('"')
            return output.toString()
        }
    }
}
