package dev.openforge

import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.project.Project
import com.intellij.openapi.wm.ToolWindow
import com.intellij.openapi.wm.ToolWindowFactory
import com.intellij.ui.components.JBScrollPane
import com.intellij.ui.components.JBTabbedPane
import com.intellij.ui.content.ContentFactory
import java.awt.BorderLayout
import java.awt.FlowLayout
import java.awt.GridLayout
import javax.swing.BorderFactory
import javax.swing.JButton
import javax.swing.JComboBox
import javax.swing.JLabel
import javax.swing.JPanel
import javax.swing.JPasswordField
import javax.swing.JTextArea
import javax.swing.JTextField
import javax.swing.SwingUtilities

class OpenForgeToolWindowFactory : ToolWindowFactory {
    override fun createToolWindowContent(project: Project, toolWindow: ToolWindow) {
        val service =
            ApplicationManager.getApplication().getService(OpenForgeService::class.java)
        val tabs = JBTabbedPane()

        val runId = JTextField()
        val controlOutput = outputArea()
        val tasksOutput = outputArea()
        val searchOutput = outputArea()
        val agentsOutput = outputArea()

        tabs.addTab(
            "Control",
            controlPanel(
                project,
                service,
                runId,
                controlOutput,
                tasksOutput,
            ),
        )
        tabs.addTab("Tasks", tasksPanel(service, runId, tasksOutput))
        tabs.addTab("Search", searchPanel(project, service, searchOutput))
        tabs.addTab("Agents", agentsPanel(service, agentsOutput))

        val root = JPanel(BorderLayout())
        root.add(tabs, BorderLayout.CENTER)
        toolWindow.contentManager.addContent(
            ContentFactory.getInstance().createContent(root, "OpenForge", false),
        )
    }

    private fun controlPanel(
        project: Project,
        service: OpenForgeService,
        runId: JTextField,
        output: JTextArea,
        tasksOutput: JTextArea,
    ): JPanel {
        val root = JPanel(BorderLayout(8, 8))
        root.border = BorderFactory.createEmptyBorder(8, 8, 8, 8)

        val fields = JPanel(GridLayout(0, 1, 5, 5))
        val daemon = JTextField(service.daemonUrl)
        val token = JPasswordField(service.apiToken().orEmpty())
        val objective = JTextArea(4, 30).apply {
            lineWrap = true
            wrapStyleWord = true
        }
        val autonomy =
            JComboBox(arrayOf("observe", "suggest", "edit", "execute", "autonomous"))
                .apply {
                    selectedItem = "edit"
                }
        val budget = JTextField("5")

        fields.add(labeled("Daemon URL", daemon))
        fields.add(labeled("API / team token", token))
        fields.add(labeled("Run ID", runId))
        fields.add(labeled("Autonomy", autonomy))
        fields.add(labeled("Hard budget USD", budget))

        val objectivePanel = JPanel(BorderLayout(5, 5))
        objectivePanel.add(JLabel("Engineering objective"), BorderLayout.NORTH)
        objectivePanel.add(JBScrollPane(objective), BorderLayout.CENTER)

        val buttons = JPanel(FlowLayout(FlowLayout.LEFT, 5, 0))
        val connect = JButton("Connect")
        val audit = JButton("Verify audit")
        val create = JButton("Create + plan")
        val execute = JButton("Execute")
        val refresh = JButton("Refresh tasks")

        buttons.add(connect)
        buttons.add(audit)
        buttons.add(create)
        buttons.add(execute)
        buttons.add(refresh)

        connect.addActionListener {
            service.daemonUrl = daemon.text.trim().ifEmpty { "http://127.0.0.1:8765" }
            service.setApiToken(String(token.password).trim().ifEmpty { null })
            runAsync(output, connect) { service.initialize() }
        }

        audit.addActionListener {
            runAsync(output, audit) { service.verifyAudit() }
        }

        create.addActionListener {
            val repo = project.basePath
            if (repo.isNullOrBlank()) {
                output.text = "Open a project before creating an OpenForge run."
                return@addActionListener
            }
            val objectiveText = objective.text.trim()
            if (objectiveText.isEmpty()) {
                output.text = "Engineering objective is required."
                return@addActionListener
            }
            val parsedBudget = budget.text.trim().toDoubleOrNull()
            if (parsedBudget == null || parsedBudget <= 0.0) {
                output.text = "Hard budget must be a positive number."
                return@addActionListener
            }

            runAsync(output, create) {
                val created =
                    service.createRun(
                        repo,
                        objectiveText,
                        autonomy.selectedItem.toString(),
                        parsedBudget,
                    )
                val id = extractJsonString(created, "id")
                    ?: throw IllegalStateException("run/create response did not contain id")
                val planned = service.planRun(repo, id)
                SwingUtilities.invokeLater {
                    runId.text = id
                    tasksOutput.text = planned
                }
                "{\"run\":" + created + ",\"tasks\":" + planned + "}"
            }
        }

        execute.addActionListener {
            val repo = project.basePath
            val id = runId.text.trim()
            if (repo.isNullOrBlank() || id.isEmpty()) {
                output.text = "Project root and run ID are required."
                return@addActionListener
            }
            runAsync(output, execute) {
                service.executeRun(repo, id, autonomy.selectedItem == "autonomous")
            }
        }

        refresh.addActionListener {
            val id = runId.text.trim()
            if (id.isEmpty()) {
                tasksOutput.text = "Run ID is required."
                return@addActionListener
            }
            runAsync(tasksOutput, refresh) { service.tasks(id) }
        }

        val center = JPanel(BorderLayout(8, 8))
        center.add(fields, BorderLayout.NORTH)
        center.add(objectivePanel, BorderLayout.CENTER)
        center.add(buttons, BorderLayout.SOUTH)

        root.add(center, BorderLayout.NORTH)
        root.add(JBScrollPane(output), BorderLayout.CENTER)
        return root
    }

    private fun tasksPanel(
        service: OpenForgeService,
        runId: JTextField,
        output: JTextArea,
    ): JPanel {
        val root = JPanel(BorderLayout(5, 5))
        root.border = BorderFactory.createEmptyBorder(8, 8, 8, 8)
        val buttons = JPanel(FlowLayout(FlowLayout.LEFT, 5, 0))
        val refresh = JButton("Refresh")
        buttons.add(JLabel("Run ID is shared with Control"))
        buttons.add(refresh)
        refresh.addActionListener {
            val id = runId.text.trim()
            if (id.isEmpty()) {
                output.text = "Run ID is required."
            } else {
                runAsync(output, refresh) { service.tasks(id) }
            }
        }
        root.add(buttons, BorderLayout.NORTH)
        root.add(JBScrollPane(output), BorderLayout.CENTER)
        return root
    }

    private fun searchPanel(
        project: Project,
        service: OpenForgeService,
        output: JTextArea,
    ): JPanel {
        val root = JPanel(BorderLayout(5, 5))
        root.border = BorderFactory.createEmptyBorder(8, 8, 8, 8)
        val controls = JPanel(BorderLayout(5, 5))
        val query = JTextField()
        val search = JButton("Search")
        controls.add(query, BorderLayout.CENTER)
        controls.add(search, BorderLayout.EAST)

        search.addActionListener {
            val repo = project.basePath
            val text = query.text.trim()
            if (repo.isNullOrBlank() || text.isEmpty()) {
                output.text = "Project root and search query are required."
            } else {
                runAsync(output, search) { service.search(repo, text) }
            }
        }
        query.addActionListener { search.doClick() }

        root.add(controls, BorderLayout.NORTH)
        root.add(JBScrollPane(output), BorderLayout.CENTER)
        return root
    }

    private fun agentsPanel(
        service: OpenForgeService,
        output: JTextArea,
    ): JPanel {
        val root = JPanel(BorderLayout(5, 5))
        root.border = BorderFactory.createEmptyBorder(8, 8, 8, 8)
        val controls = JPanel(FlowLayout(FlowLayout.LEFT, 5, 0))
        val refresh = JButton("List agents")
        val thread = JTextField(22)
        val instruction = JTextField(30)
        val queue = JButton("Queue instruction")

        controls.add(refresh)
        controls.add(JLabel("Thread"))
        controls.add(thread)
        controls.add(JLabel("Instruction"))
        controls.add(instruction)
        controls.add(queue)

        refresh.addActionListener {
            runAsync(output, refresh) { service.agents() }
        }
        queue.addActionListener {
            val threadId = thread.text.trim()
            val message = instruction.text.trim()
            if (threadId.isEmpty() || message.isEmpty()) {
                output.text = "Thread ID and instruction are required."
            } else {
                runAsync(output, queue) {
                    service.queueInstruction(threadId, message)
                }
            }
        }

        root.add(controls, BorderLayout.NORTH)
        root.add(JBScrollPane(output), BorderLayout.CENTER)
        return root
    }

    private fun labeled(label: String, component: java.awt.Component): JPanel {
        val panel = JPanel(BorderLayout(5, 5))
        panel.add(JLabel(label), BorderLayout.WEST)
        panel.add(component, BorderLayout.CENTER)
        return panel
    }

    private fun outputArea(): JTextArea =
        JTextArea().apply {
            isEditable = false
            lineWrap = false
            tabSize = 2
        }

    private fun runAsync(
        output: JTextArea,
        button: JButton,
        operation: () -> String,
    ) {
        button.isEnabled = false
        ApplicationManager.getApplication().executeOnPooledThread {
            val result = runCatching(operation)
            SwingUtilities.invokeLater {
                output.text =
                    result.fold(
                        onSuccess = { it },
                        onFailure = { error -> "OpenForge request failed: " + error.message },
                    )
                output.caretPosition = 0
                button.isEnabled = true
            }
        }
    }

    private fun extractJsonString(payload: String, field: String): String? {
        val pattern =
            Regex(
                "\"" +
                    Regex.escape(field) +
                    "\"\\s*:\\s*\"((?:\\\\.|[^\"])*)\"",
            )
        val encoded = pattern.find(payload)?.groupValues?.getOrNull(1) ?: return null
        return unescapeJsonString(encoded)
    }

    private fun unescapeJsonString(value: String): String {
        val output = StringBuilder(value.length)
        var index = 0
        while (index < value.length) {
            val character = value[index]
            if (character != '\\' || index + 1 >= value.length) {
                output.append(character)
                index += 1
                continue
            }
            index += 1
            when (val escaped = value[index]) {
                '"' -> output.append('"')
                '\\' -> output.append('\\')
                '/' -> output.append('/')
                'b' -> output.append('\b')
                'f' -> output.append('\u000C')
                'n' -> output.append('\n')
                'r' -> output.append('\r')
                't' -> output.append('\t')
                'u' -> {
                    if (index + 4 >= value.length) {
                        throw IllegalArgumentException("invalid JSON unicode escape")
                    }
                    val code = value.substring(index + 1, index + 5).toInt(16)
                    output.append(code.toChar())
                    index += 4
                }
                else -> throw IllegalArgumentException("invalid JSON escape: $escaped")
            }
            index += 1
        }
        return output.toString()
    }
}
