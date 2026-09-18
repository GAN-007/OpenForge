package dev.openforge

import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.project.Project
import com.intellij.openapi.wm.ToolWindow
import com.intellij.openapi.wm.ToolWindowFactory
import com.intellij.ui.content.ContentFactory
import java.awt.BorderLayout
import javax.swing.JButton
import javax.swing.JPanel
import javax.swing.JScrollPane
import javax.swing.JTextArea
import javax.swing.SwingUtilities

class OpenForgeToolWindowFactory : ToolWindowFactory {
    override fun createToolWindowContent(project: Project, toolWindow: ToolWindow) {
        val service =
            ApplicationManager.getApplication().getService(OpenForgeService::class.java)

        val panel = JPanel(BorderLayout(8, 8))
        val status =
            JTextArea("OpenForge daemon not queried yet").apply {
                isEditable = false
                lineWrap = true
                wrapStyleWord = true
            }
        val button = JButton("Connect")

        button.addActionListener {
            button.isEnabled = false
            Thread {
                val result = runCatching { service.rpc("initialize") }
                SwingUtilities.invokeLater {
                    status.text =
                        result.fold(
                            onSuccess = { it },
                            onFailure = { error ->
                                "Connection failed: " + error.message
                            },
                        )
                    button.isEnabled = true
                }
            }.start()
        }

        panel.add(button, BorderLayout.NORTH)
        panel.add(JScrollPane(status), BorderLayout.CENTER)

        toolWindow.contentManager.addContent(
            ContentFactory.getInstance()
                .createContent(panel, "Control Plane", false)
        )
    }
}
