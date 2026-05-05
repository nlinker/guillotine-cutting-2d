Attribute VB_Name = "checkboxes"

Sub CreateCheckboxes()
    Dim ws As Worksheet
    Dim cb As CheckBox
    Dim cell As Range
    Dim i As Integer
    Dim colWidth As Double

    Set ws = ActiveSheet
    colWidth = ws.Columns(6).Width  ' F column width

    ' Remove existing checkboxes to avoid duplicates on re-run
    Dim shp As Object
    For Each shp In ws.CheckBoxes
        shp.Delete
    Next shp

    ' Main checkbox in F3, linked to E3
    Set cell = ws.Cells(3, 6)
    Set cb = ws.CheckBoxes.Add(cell.Left, cell.Top, colWidth, cell.Height)
    cb.Caption = "all"
    cb.OnAction = "checkboxes.MainCheckboxClick"
    cb.Name = "cbMain"

    ' Individual checkboxes in F4:F103, linked to E4:E103
    For i = 4 To 103
        Set cell = ws.Cells(i, 6)
        Set cb = ws.CheckBoxes.Add(cell.Left, cell.Top, colWidth, cell.Height)
        cb.LinkedCell = ws.Cells(i, 5).Address
        cb.Caption = "may rotate?"
        cb.Name = "cbRow" & i
    Next i
End Sub

Sub MainCheckboxClick()
    Dim ws As Worksheet
    Dim mainVal As Boolean
    Dim i As Integer

    Set ws = ActiveSheet
    mainVal = (ws.CheckBoxes("cbMain").Value = xlOn)

    For i = 4 To 103
        If ws.Cells(i, 5).Value <> "" Then
            ws.Cells(i, 5).Value = mainVal
        End If
    Next i
End Sub
