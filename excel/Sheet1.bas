'' Sheet1 module — event handlers for Sheet1 (cut layout worksheet).
''
'' MANUAL SETUP REQUIRED after importing this file:
''   1. Add a Spinner control next to cell L1 (the seed cell):
''      Developer tab → Insert → Form Controls → Spinner
''      Draw it next to L1, then right-click → Format Control:
''        - Cell link: $L$1
''        - Minimum value: 0
''        - Maximum value: 32767  (or any upper bound you prefer)
''        - Incremental change: 1
''   2. Assign the restart macro to the Spinner:
''      Right-click the Spinner → Assign Macro → select "cut.RestartCut"
''      NOTE: Forms Spinner also fires Worksheet_Change when it updates the
''      linked cell, so L1 is intentionally excluded here to avoid double invocation.

Private Sub Worksheet_Change(ByVal Target As Range)
    ' L1 (seed) is handled by the Spinner's assigned macro (cut.RestartCut).
    ' Handling it here too would cause double invocation and pipe conflict.
End Sub
