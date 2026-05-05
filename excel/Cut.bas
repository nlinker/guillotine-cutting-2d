Attribute VB_Name = "cut"
''
'' cut.bas  —  VBA module: launches cut.exe and reads progress via named pipe
''
'' Sheet layout ("Sheet1"):
''   B1        : path to cut.exe
''   H1        : stock sheet width  (mm)
''   I1        : stock sheet height (mm)
''   I2        : blade kerf          (mm)
''   A4:E*     : pieces — name, width, height, count, can_rotate (until B or C is empty)
''
'' Progress output (right side of sheet):
''   K1 — status     K2 — generation     K3 — objective     K4 — sheets used
''
'' Results (from K6): placement table — sheet | name | w×h | x | y | rotated
''
'' Requires JsonConverter module (excel/JsonConverter.bas) imported into the workbook.
''
'' Usage:
''   1. Alt+F11 → Import File → cut.bas + JsonConverter.bas
''   2. Fill in data on Sheet1
''   3. Run macro RunCut
'' ═══════════════════════════════════════════════════════════════════════════

Option Explicit

'' == Windows API =============================================================

Private Declare PtrSafe Function CreateFile Lib "kernel32" Alias "CreateFileW" ( _
    ByVal lpFileName       As LongPtr, _
    ByVal dwDesiredAccess  As Long, _
    ByVal dwShareMode      As Long, _
    ByVal lpSecurityAttrib As LongPtr, _
    ByVal dwCreationDisp   As Long, _
    ByVal dwFlagsAndAttrib As Long, _
    ByVal hTemplateFile    As LongPtr _
) As LongPtr

Private Declare PtrSafe Function ReadFile Lib "kernel32" ( _
    ByVal hFile                As LongPtr, _
    ByVal lpBuffer             As LongPtr, _
    ByVal nNumberOfBytesToRead As Long, _
    ByRef lpNumberOfBytesRead  As Long, _
    ByVal lpOverlapped         As LongPtr _
) As Long

Private Declare PtrSafe Function CloseHandle Lib "kernel32" ( _
    ByVal hObject As LongPtr _
) As Long

Private Declare PtrSafe Function WaitNamedPipe Lib "kernel32" Alias "WaitNamedPipeW" ( _
    ByVal lpNamedPipeName As LongPtr, _
    ByVal nTimeOut        As Long _
) As Long

Private Declare PtrSafe Sub Sleep Lib "kernel32" ( _
    ByVal dwMilliseconds As Long _
)

'' == Constants ================================================================

Private Const PIPE_NAME        As String  = "\\.\pipe\cut_progress"
Private Const GENERIC_READ     As Long    = &H80000000
Private Const OPEN_EXISTING    As Long    = 3
Private Const FILE_ATTR_NORMAL As Long    = &H80
Private Const INVALID_HANDLE   As LongPtr = -1
Private Const BUFFER_SIZE      As Long    = 8192

Private Const DATA_START_ROW   As Long = 4   ' first piece row
Private Const OUT_COL          As Long = 11  ' column K (1-based) for progress output
Private Const RESULT_ROW       As Long = 6   ' first row of placement results table

'' == State ====================================================================

Private g_Running As Boolean

'' == Helpers ==================================================================

' Escapes a string for JSON: non-ASCII and control chars become \uXXXX.
' Ported from ../cutting/vba/cut_api.bas.
Private Function JsonEscapeStr(s As String) As String
    Dim result As String
    Dim i As Integer
    Dim c As Long
    result = ""
    For i = 1 To Len(s)
        c = AscW(Mid(s, i, 1))
        If c < 0 Then c = c + 65536
        Select Case c
            Case 34:      result = result & "\"""
            Case 92:      result = result & "\\"
            Case Is < 32: result = result & "\u" & Right("0000" & Hex(c), 4)
            Case Is > 127:result = result & "\u" & Right("0000" & Hex(c), 4)
            Case Else:    result = result & Chr(c)
        End Select
    Next i
    JsonEscapeStr = result
End Function

Private Function WStrPtr(s As String) As LongPtr
    WStrPtr = StrPtr(s)
End Function

' Writes progress values to column K.
Private Sub SetProgress(ws As Worksheet, status As String, gen As String, _
                        obj As String, sheets As String)
    ws.Cells(1, OUT_COL).Value = status
    If gen    <> "" Then ws.Cells(2, OUT_COL).Value = CLng(gen)
    If obj    <> "" Then ws.Cells(3, OUT_COL).Value = CDbl(obj)
    If sheets <> "" Then ws.Cells(4, OUT_COL).Value = CLng(sheets)
    DoEvents
End Sub

' Writes progress labels to column J and clears previous results.
Private Sub InitOutputArea(ws As Worksheet)
    Dim labCol As Long
    labCol = OUT_COL - 1  ' column J

    ws.Cells(1, labCol).Value = "Status"
    ws.Cells(2, labCol).Value = "Generation"
    ws.Cells(3, labCol).Value = "Objective"
    ws.Cells(4, labCol).Value = "Sheets"

    ws.Range(ws.Cells(RESULT_ROW, OUT_COL - 1), ws.Cells(1000, OUT_COL + 5)).ClearContents
End Sub

' Renders the placement table after a Done message.
Private Sub RenderPlacements(ws As Worksheet, sol As Object, pieces As Object)
    Dim r As Long
    r = RESULT_ROW
    Dim labCol As Long
    labCol = OUT_COL - 1

    ' Headers
    ws.Cells(r, labCol).Value      = "Sheet"
    ws.Cells(r, OUT_COL).Value     = "Piece"
    ws.Cells(r, OUT_COL + 1).Value = "Width"
    ws.Cells(r, OUT_COL + 2).Value = "Height"
    ws.Cells(r, OUT_COL + 3).Value = "X"
    ws.Cells(r, OUT_COL + 4).Value = "Y"
    ws.Cells(r, OUT_COL + 5).Value = "Rotated"
    r = r + 1

    Dim pl As Object
    For Each pl In sol("placements")
        Dim idx As Long
        idx = pl("piece_idx") + 1  ' VBA Collection is 1-based

        Dim pieceName As String
        pieceName = pieces(idx)("name")

        Dim pw As Long, ph As Long
        If pl("rotated") Then
            pw = pieces(idx)("height")
            ph = pieces(idx)("width")
        Else
            pw = pieces(idx)("width")
            ph = pieces(idx)("height")
        End If

        ws.Cells(r, labCol).Value      = pl("sheet_idx")
        ws.Cells(r, OUT_COL).Value     = pieceName
        ws.Cells(r, OUT_COL + 1).Value = pw
        ws.Cells(r, OUT_COL + 2).Value = ph
        ws.Cells(r, OUT_COL + 3).Value = pl("x")
        ws.Cells(r, OUT_COL + 4).Value = pl("y")
        ws.Cells(r, OUT_COL + 5).Value = IIf(pl("rotated"), "yes", "")
        r = r + 1
    Next pl
    DoEvents
End Sub

'' == JSON builder =============================================================

Private Function BuildProblemJson(ws As Worksheet) As String
    Dim sheetWidth  As Long: sheetWidth  = ws.Cells(1, 8).Value  ' H1
    Dim sheetHeight As Long: sheetHeight = ws.Cells(1, 9).Value  ' I1
    Dim kerf        As Long: kerf        = ws.Cells(2, 9).Value  ' I2

    Dim sPieces As String
    Dim bFirst  As Boolean: bFirst = True
    Dim i As Long: i = DATA_START_ROW

    Do
        Dim w As Long, h As Long
        w = 0: h = 0
        If ws.Cells(i, 2).Value <> "" Then w = CLng(ws.Cells(i, 2).Value)
        If ws.Cells(i, 3).Value <> "" Then h = CLng(ws.Cells(i, 3).Value)
        If w = 0 Or h = 0 Then Exit Do

        Dim pName   As String:  pName   = Trim(ws.Cells(i, 1).Value)
        Dim pCount  As Long:    pCount  = CLng(ws.Cells(i, 4).Value)
        Dim pRotate As Boolean: pRotate = (ws.Cells(i, 5).Value = True)

        Dim sPiece As String
        sPiece = "{""name"":"""  & JsonEscapeStr(pName) & """" & _
                 ",""width"":"  & CStr(w) & _
                 ",""height"":" & CStr(h) & _
                 ",""count"":"  & CStr(pCount) & _
                 ",""can_rotate"":" & IIf(pRotate, "true", "false") & "}"

        If bFirst Then
            sPieces = sPiece
            bFirst = False
        Else
            sPieces = sPieces & "," & sPiece
        End If
        i = i + 1
    Loop

    BuildProblemJson = "{""sheet"":{""width"":" & CStr(sheetWidth) & _
                       ",""height"":" & CStr(sheetHeight) & "}" & _
                       ",""kerf"":" & CStr(kerf) & _
                       ",""pieces"":[" & sPieces & "]}"
End Function

'' == Main macro ===============================================================

Public Sub RunCut()
    If g_Running Then
        MsgBox "Solver is already running!", vbInformation
        Exit Sub
    End If

    Dim ws As Worksheet
    Set ws = ThisWorkbook.Sheets("Sheet1")

    Dim exePath As String
    exePath = Trim(ws.Cells(1, 2).Value)  ' B1
    If Dir(exePath) = "" Then
        MsgBox "cut.exe not found: " & exePath & Chr(13) & _
               "Set the correct path in cell B1.", vbCritical
        Exit Sub
    End If

    InitOutputArea ws
    SetProgress ws, "Preparing...", "", "", ""

    ' Build JSON and write to a temp file
    Dim jsonStr As String
    jsonStr = BuildProblemJson(ws)

    Dim tmpFile As String
    tmpFile = Environ("TEMP") & "\cut_input.json"

    Dim fNum As Integer
    fNum = FreeFile
    Open tmpFile For Output As #fNum
    Print #fNum, jsonStr
    Close #fNum

    ' Launch cut.exe (non-blocking Shell)
    Dim cmd As String
    cmd = Chr(34) & exePath & Chr(34) & " calc --json " & Chr(34) & tmpFile & Chr(34)
    Shell cmd, vbHide

    ' Give cut.exe time to create the pipe
    Sleep 800

    ' Connect to the named pipe (with retries)
    Dim pipeName As String
    pipeName = PIPE_NAME

    Dim hPipe As LongPtr
    Dim attempt As Integer
    For attempt = 1 To 15
        WaitNamedPipe WStrPtr(pipeName & Chr(0)), 2000
        hPipe = CreateFile( _
            WStrPtr(pipeName & Chr(0)), _
            GENERIC_READ, 0, 0, OPEN_EXISTING, FILE_ATTR_NORMAL, 0)
        If hPipe <> INVALID_HANDLE Then Exit For
        Sleep 400
        DoEvents
    Next attempt

    If hPipe = INVALID_HANDLE Then
        MsgBox "Could not connect to named pipe." & Chr(13) & _
               "Make sure cut.exe started successfully.", vbCritical
        SetProgress ws, "Connection error", "", "", ""
        Exit Sub
    End If

    '' == Message read loop ====================================================
    g_Running = True
    SetProgress ws, "Running...", "", "", ""

    Dim buf()    As Byte
    Dim nRead    As Long
    Dim raw      As String
    Dim leftover As String
    ReDim buf(BUFFER_SIZE - 1)
    leftover = ""

    Do While g_Running
        nRead = 0
        Dim ok As Long
        ok = ReadFile(hPipe, VarPtr(buf(0)), BUFFER_SIZE, nRead, 0)

        If ok = 0 Or nRead = 0 Then Exit Do   ' pipe closed

        ' Convert bytes to string (messages are ASCII-compatible;
        ' non-ASCII piece names arrive as \uXXXX escapes from serde_json)
        raw = leftover
        Dim b As Long
        For b = 0 To nRead - 1
            raw = raw & Chr(buf(b))
        Next b
        leftover = ""

        ' Split into lines
        Dim lines() As String
        lines = Split(raw, Chr(10))

        Dim li As Integer
        For li = 0 To UBound(lines)
            Dim ln As String
            ln = Trim(lines(li))
            If ln = "" Then GoTo NextLine

            ' Last element from Split without trailing \n — incomplete line
            If li = UBound(lines) And Right(raw, 1) <> Chr(10) Then
                leftover = ln
                GoTo NextLine
            End If

            ' Parse JSON line
            On Error Resume Next
            Dim msg As Object
            Set msg = JsonConverter.ParseJson(ln)
            On Error GoTo 0

            If msg Is Nothing Then GoTo NextLine

            Select Case msg("type")
                Case "progress"
                    SetProgress ws, "Running...", _
                        CStr(msg("generation")), _
                        CStr(msg("objective")), _
                        CStr(msg("sheets_used"))

                Case "done"
                    SetProgress ws, "Done " & Chr(10003), "", _
                        CStr(msg("objective")), _
                        CStr(msg("sheets_used"))
                    RenderPlacements ws, msg("solution"), msg("pieces")
                    g_Running = False

                Case "error"
                    SetProgress ws, "Error: " & msg("message"), "", "", ""
                    g_Running = False
            End Select

            Set msg = Nothing
NextLine:
        Next li

        DoEvents
        Sleep 100
    Loop

    CloseHandle hPipe
    g_Running = False
End Sub

'' == Stop =====================================================================

Public Sub StopCut()
    g_Running = False
    ThisWorkbook.Sheets("Sheet1").Cells(1, OUT_COL).Value = "Stopped"
End Sub
