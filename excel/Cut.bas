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
'' ============================================================================

Option Explicit

'' == Windows API =============================================================

#If VBA7 Then
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
#Else
    Private Declare Function CreateFile Lib "kernel32" Alias "CreateFileW" ( _
        ByVal lpFileName       As Long, _
        ByVal dwDesiredAccess  As Long, _
        ByVal dwShareMode      As Long, _
        ByVal lpSecurityAttrib As Long, _
        ByVal dwCreationDisp   As Long, _
        ByVal dwFlagsAndAttrib As Long, _
        ByVal hTemplateFile    As Long _
    ) As Long
    Private Declare Function ReadFile Lib "kernel32" ( _
        ByVal hFile                As Long, _
        ByVal lpBuffer             As Long, _
        ByVal nNumberOfBytesToRead As Long, _
        ByRef lpNumberOfBytesRead  As Long, _
        ByVal lpOverlapped         As Long _
    ) As Long
    Private Declare Function CloseHandle Lib "kernel32" ( _
        ByVal hObject As Long _
    ) As Long
    Private Declare Function WaitNamedPipe Lib "kernel32" Alias "WaitNamedPipeW" ( _
        ByVal lpNamedPipeName As Long, _
        ByVal nTimeOut        As Long _
    ) As Long
    Private Declare Sub Sleep Lib "kernel32" ( _
        ByVal dwMilliseconds As Long _
    )
#End If

'' == Constants ================================================================

Private Const SHEET_NAME       As String  = "Sheet1"
Private Const PIPE_NAME        As String  = "\\.\pipe\cut_progress"
Private Const GENERIC_READ     As Long    = &H80000000
Private Const OPEN_EXISTING    As Long    = 3
Private Const FILE_ATTR_NORMAL As Long    = &H80
#If VBA7 Then
    Private Const INVALID_HANDLE As LongPtr = -1
#Else
    Private Const INVALID_HANDLE As Long = -1
#End If
Private Const BUFFER_SIZE      As Long    = 8192

Private Const DATA_COL         As Long = 1   ' leftmost column of piece table (Name)
Private Const DATA_HEADER_ROW  As Long = 5   ' piece table header row (Name, Width, Height, Count, Rotate)

Private Const CFG_ROW          As Long = 1   ' row for GA config cells
Private Const CFG_THREADS_CELL As Long = 3   ' C1 — parallel threads (--threads)
Private Const CFG_GENS_CELL    As Long = 4   ' D1 — generations per run (--gens)
Private Const CFG_POP_CELL     As Long = 5   ' E1 — population size (--pop)
Private Const OUT_COL          As Long = 11  ' column K (1-based) for progress output
Private Const RESULT_ROW       As Long = 6   ' first row of placement results table

Private Const CANVAS_COL       As Long   = 7     ' column G — drawing canvas left edge
Private Const CANVAS_ROW       As Long   = 5     ' row 5 — drawing canvas top edge
Private Const CANVAS_END_COL   As Long   = 12    ' column L — right boundary (drawing fills G:K)
Private Const CANVAS_SHEET_GAP As Double = 14#   ' gap between sheets in points
Private Const RESULT_COL       As Long   = 14    ' column N — placement table data column (label "Sheet" in M)

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

#If VBA7 Then
Private Function WStrPtr(s As String) As LongPtr
    WStrPtr = StrPtr(s)
End Function
#Else
Private Function WStrPtr(s As String) As Long
    WStrPtr = StrPtr(s)
End Function
#End If

' Decodes a slice of a byte array from UTF-8 to a VBA Unicode string.
Private Function Utf8ToStr(buf() As Byte, nBytes As Long) As String
    If nBytes = 0 Then Utf8ToStr = "": Exit Function
    Dim tmp() As Byte
    ReDim tmp(nBytes - 1)
    Dim i As Long
    For i = 0 To nBytes - 1
        tmp(i) = buf(i)
    Next i
    With CreateObject("ADODB.Stream")
        .Open
        .Type = 1        ' adTypeBinary
        .Write tmp
        .Position = 0
        .Type = 2        ' adTypeText
        .Charset = "UTF-8"
        Utf8ToStr = .ReadText
        .Close
    End With
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

    ws.Range(ws.Cells(RESULT_ROW, RESULT_COL - 1), ws.Cells(1000, RESULT_COL + 5)).ClearContents
    ClearLayoutShapes ws
    ClearPieceColors ws
End Sub

' Renders the placement table after a Done message.
Private Sub RenderPlacements(ws As Worksheet, sol As Object, pieces As Object)
    Dim r As Long
    r = RESULT_ROW
    Dim labCol As Long
    labCol = RESULT_COL - 1

    ' Headers
    ws.Cells(r, labCol).Value          = "Sheet"
    ws.Cells(r, RESULT_COL).Value      = "Piece"
    ws.Cells(r, RESULT_COL + 1).Value  = "Width"
    ws.Cells(r, RESULT_COL + 2).Value  = "Height"
    ws.Cells(r, RESULT_COL + 3).Value  = "X"
    ws.Cells(r, RESULT_COL + 4).Value  = "Y"
    ws.Cells(r, RESULT_COL + 5).Value  = "Rotated"
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

        ws.Cells(r, labCol).Value          = pl("sheet_idx")
        ws.Cells(r, RESULT_COL).Value      = pieceName
        ws.Cells(r, RESULT_COL + 1).Value  = pw
        ws.Cells(r, RESULT_COL + 2).Value  = ph
        ws.Cells(r, RESULT_COL + 3).Value  = pl("x")
        ws.Cells(r, RESULT_COL + 4).Value  = pl("y")
        ws.Cells(r, RESULT_COL + 5).Value  = IIf(pl("rotated"), "yes", "")
        r = r + 1
    Next pl
    DoEvents
End Sub

'' == Layout drawing ===========================================================

' Maps piece name to a fill color from a 12-color palette.
Private Function PieceColor(ByVal pieceName As String) As Long
    Dim p(11) As Long
    p(0)  = RGB(255, 182, 193): p(1)  = RGB(173, 216, 230)
    p(2)  = RGB(144, 238, 144): p(3)  = RGB(255, 255, 153)
    p(4)  = RGB(255, 200, 120): p(5)  = RGB(221, 160, 221)
    p(6)  = RGB(135, 206, 235): p(7)  = RGB(240, 180, 180)
    p(8)  = RGB(180, 255, 180): p(9)  = RGB(255, 228, 196)
    p(10) = RGB(200, 200, 255): p(11) = RGB(255, 240, 180)
    Dim h As Long: h = 0
    Dim i As Integer
    For i = 1 To Len(pieceName)
        h = (h * 31 + AscW(Mid(pieceName, i, 1))) Mod 12
    Next i
    PieceColor = p(Abs(h) Mod 12)
End Function

' Deletes all shapes whose name starts with "cut_".
Private Sub ClearLayoutShapes(ws As Worksheet)
    Dim shapeNames() As String
    Dim n As Long: n = 0
    Dim shp As Shape
    For Each shp In ws.Shapes
        If Left(shp.Name, 4) = "cut_" Then
            ReDim Preserve shapeNames(n)
            shapeNames(n) = shp.Name
            n = n + 1
        End If
    Next shp
    Dim i As Long
    For i = 0 To n - 1
        ws.Shapes(shapeNames(i)).Delete
    Next i
End Sub

' Draws cutting layout as Excel shapes starting at column CANVAS_COL, row 1.
' Sheets are displayed side-by-side; pieces are color-coded by name.
Private Sub DrawLayout(ws As Worksheet, sol As Object, pieces As Object, _
                       sheetW As Long, sheetH As Long)
    If sheetW <= 0 Or sheetH <= 0 Then Exit Sub

    ClearLayoutShapes ws

    Dim originLeft  As Double: originLeft  = ws.Cells(CANVAS_ROW, CANVAS_COL).Left
    Dim originTop   As Double: originTop   = ws.Cells(CANVAS_ROW, CANVAS_COL).Top
    Dim sheetsDispW As Double: sheetsDispW = ws.Cells(1, CANVAS_END_COL).Left - originLeft
    Dim scl         As Double: scl         = sheetsDispW / sheetW
    Dim sheetDispH  As Double: sheetDispH  = sheetH * scl

    ' Count sheets
    Dim nSheets As Long: nSheets = 0
    Dim pl As Object
    For Each pl In sol("placements")
        If pl("sheet_idx") + 1 > nSheets Then nSheets = pl("sheet_idx") + 1
    Next pl

    ' Draw sheet backgrounds (stacked vertically)
    Dim si As Long
    For si = 0 To nSheets - 1
        Dim bgTop As Double: bgTop = originTop + si * (sheetDispH + CANVAS_SHEET_GAP)
        Dim bg As Shape
        Set bg = ws.Shapes.AddShape(msoShapeRectangle, originLeft, bgTop, _
                                    sheetsDispW, sheetDispH)
        bg.Name = "cut_bg_" & si
        bg.Fill.ForeColor.RGB = RGB(248, 248, 248)
        bg.Fill.Transparency = 0
        bg.Line.ForeColor.RGB = RGB(60, 60, 60)
        bg.Line.Weight = 1#
        bg.TextFrame2.TextRange.Text = "Sheet " & si
        bg.TextFrame2.TextRange.Font.Size = 8
    Next si

    ' Draw pieces
    For Each pl In sol("placements")
        Dim idx   As Long: idx   = pl("piece_idx") + 1
        Dim shIdx As Long: shIdx = pl("sheet_idx")

        Dim pw As Long, ph As Long
        If pl("rotated") Then
            pw = pieces(idx)("height"): ph = pieces(idx)("width")
        Else
            pw = pieces(idx)("width"):  ph = pieces(idx)("height")
        End If

        Dim rLeft   As Double: rLeft   = originLeft + pl("x") * scl
        Dim rTop    As Double: rTop    = originTop + shIdx * (sheetDispH + CANVAS_SHEET_GAP) + pl("y") * scl
        Dim rWidth  As Double: rWidth  = pw * scl
        Dim rHeight As Double: rHeight = ph * scl
        If rWidth  < 1# Then rWidth  = 1#
        If rHeight < 1# Then rHeight = 1#

        Dim s As Shape
        Set s = ws.Shapes.AddShape(msoShapeRectangle, rLeft, rTop, rWidth, rHeight)
        s.Name = "cut_p_" & shIdx & "_" & (idx - 1)
        s.Fill.ForeColor.RGB = PieceColor(pieces(idx)("name"))
        s.Fill.Transparency = 0
        s.Line.ForeColor.RGB = RGB(80, 80, 80)
        s.Line.Weight = 0.5#

        If rWidth >= 20# And rHeight >= 12# Then
            s.TextFrame2.TextRange.Text = pieces(idx)("name")
            s.TextFrame2.TextRange.Font.Size = 7
            s.TextFrame2.TextRange.Font.Fill.ForeColor.RGB = RGB(0, 0, 0)
            s.TextFrame2.WordWrap = msoFalse
        Else
            s.TextFrame2.TextRange.Text = ""
        End If
    Next pl
End Sub

'' == Piece row coloring =======================================================

Private Sub ClearPieceColors(ws As Worksheet)
    ws.Range(ws.Cells(DATA_HEADER_ROW + 1, DATA_COL), ws.Cells(200, DATA_COL + 4)).Interior.ColorIndex = xlNone
End Sub

Private Sub ColorPieceRows(ws As Worksheet, pieces As Object)
    Dim i As Long
    Dim pIdx As Long: pIdx = 1  ' 1-based index into expanded pieces array
    For i = DATA_HEADER_ROW + 1 To 200
        Dim w As Long, h As Long
        w = 0: h = 0
        If ws.Cells(i, DATA_COL + 1).Value <> "" Then w = CLng(ws.Cells(i, DATA_COL + 1).Value)
        If ws.Cells(i, DATA_COL + 2).Value <> "" Then h = CLng(ws.Cells(i, DATA_COL + 2).Value)
        If w = 0 Or h = 0 Then Exit For
        Dim cnt As Long: cnt = 1
        If ws.Cells(i, DATA_COL + 3).Value <> "" Then cnt = CLng(ws.Cells(i, DATA_COL + 3).Value)
        If cnt < 1 Then cnt = 1
        Dim pName As String: pName = pieces(pIdx)("name")
        ws.Range(ws.Cells(i, DATA_COL), ws.Cells(i, DATA_COL + 4)).Interior.Color = PieceColor(pName)
        pIdx = pIdx + cnt
    Next i
End Sub

'' == JSON builder =============================================================

Private Function BuildProblemJson(ws As Worksheet) As String
    Dim sheetWidth  As Long: sheetWidth  = ws.Cells(1, 8).Value  ' H1
    Dim sheetHeight As Long: sheetHeight = ws.Cells(1, 9).Value  ' I1
    Dim kerf        As Long: kerf        = ws.Cells(2, 9).Value  ' I2

    Dim sPieces As String
    Dim bFirst  As Boolean: bFirst = True
    Dim i As Long: i = DATA_HEADER_ROW + 1

    Do
        Dim w As Long, h As Long
        w = 0: h = 0
        If ws.Cells(i, DATA_COL + 1).Value <> "" Then w = CLng(ws.Cells(i, DATA_COL + 1).Value)
        If ws.Cells(i, DATA_COL + 2).Value <> "" Then h = CLng(ws.Cells(i, DATA_COL + 2).Value)
        If w = 0 Or h = 0 Then Exit Do

        Dim pName   As String:  pName   = Trim(ws.Cells(i, DATA_COL).Value)
        Dim pCount  As Long:    pCount  = CLng(ws.Cells(i, DATA_COL + 3).Value)
        Dim pRotate As Boolean: pRotate = (ws.Cells(i, DATA_COL + 4).Value = True)

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
    Set ws = ThisWorkbook.Sheets(SHEET_NAME)

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

    ' Read GA config from sheet (use defaults if cells empty)
    Dim nThreads As Long: nThreads = 4
    Dim nGens    As Long: nGens    = 2000
    Dim nPop     As Long: nPop     = 200
    If ws.Cells(CFG_ROW, CFG_THREADS_CELL).Value <> "" Then nThreads = CLng(ws.Cells(CFG_ROW, CFG_THREADS_CELL).Value)
    If ws.Cells(CFG_ROW, CFG_GENS_CELL).Value    <> "" Then nGens    = CLng(ws.Cells(CFG_ROW, CFG_GENS_CELL).Value)
    If ws.Cells(CFG_ROW, CFG_POP_CELL).Value     <> "" Then nPop     = CLng(ws.Cells(CFG_ROW, CFG_POP_CELL).Value)

    ' Launch cut.exe (non-blocking Shell)
    Dim cmd As String
    cmd = Chr(34) & exePath & Chr(34) & " calc --json " & Chr(34) & tmpFile & Chr(34) _
        & " --threads " & nThreads & " --gens " & nGens & " --pop " & nPop
    Shell cmd, vbHide

    ' Give cut.exe time to create the pipe
    Sleep 800

    ' Connect to the named pipe (with retries)
    Dim pipeName As String
    pipeName = PIPE_NAME

    #If VBA7 Then
        Dim hPipe As LongPtr
    #Else
        Dim hPipe As Long
    #End If
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

        raw = leftover & Utf8ToStr(buf, nRead)
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
                    SetProgress ws, "Done " & ChrW(10003), "", _
                        CStr(msg("objective")), _
                        CStr(msg("sheets_used"))
                    Application.ScreenUpdating = False
                    ws.Range(ws.Cells(RESULT_ROW, RESULT_COL - 1), _
                             ws.Cells(1000, RESULT_COL + 5)).ClearContents
                    RenderPlacements ws, msg("solution"), msg("pieces")
                    DrawLayout ws, msg("solution"), msg("pieces"), _
                        ws.Cells(1, 8).Value, ws.Cells(1, 9).Value
                    ColorPieceRows ws, msg("pieces")
                    Application.ScreenUpdating = True
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
    ThisWorkbook.Sheets(SHEET_NAME).Cells(1, OUT_COL).Value = "Stopped"
End Sub

'' == Checkboxes for "Can rotate?" =============================================

Sub CreateCheckboxes()
    Dim ws As Worksheet
    Set ws = ThisWorkbook.Sheets(SHEET_NAME)

    Dim shp As Object
    For Each shp In ws.CheckBoxes
        shp.Delete
    Next shp

    Dim cbCol     As Long:   cbCol     = DATA_COL + 5
    Dim colWidth  As Double: colWidth  = ws.Columns(cbCol).Width
    Dim cell      As Range
    Dim cb        As CheckBox
    Dim i         As Integer

    Dim mg As Double: mg = 1#

    Set cell = ws.Cells(DATA_HEADER_ROW, cbCol)
    Set cb = ws.CheckBoxes.Add(cell.Left + mg, cell.Top + mg, colWidth - 2*mg, cell.Height - 2*mg)
    cb.Caption  = "all"
    cb.OnAction = "cut.MainCheckboxClick"
    cb.Name     = "cbMain"

    For i = DATA_HEADER_ROW + 1 To DATA_HEADER_ROW + 100
        Set cell = ws.Cells(i, cbCol)
        Set cb = ws.CheckBoxes.Add(cell.Left + mg, cell.Top + mg, colWidth - 2*mg, cell.Height - 2*mg)
        cb.LinkedCell = ws.Cells(i, DATA_COL + 4).Address
        cb.Caption    = ""
        cb.Name       = "cbRow" & i
    Next i
End Sub

Sub MainCheckboxClick()
    Dim ws As Worksheet
    Dim mainVal As Boolean
    Dim i As Integer

    Set ws = ActiveSheet
    mainVal = (ws.CheckBoxes("cbMain").Value = xlOn)

    For i = DATA_HEADER_ROW + 1 To DATA_HEADER_ROW + 100
        If ws.Cells(i, DATA_COL + 4).Value <> "" Then
            ws.Cells(i, DATA_COL + 4).Value = mainVal
        End If
    Next i
End Sub
