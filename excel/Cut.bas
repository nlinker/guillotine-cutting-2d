Attribute VB_Name = "Cut"
''
'' Cut.bas  -  VBA module: launches cut.exe and reads progress via named pipe
''
'' Requires JsonConverter module (excel/JsonConverter.bas) imported into the workbook.
''
'' Usage:
''   1. Alt+F11 -> Import File -> cut.bas + JsonConverter.bas
''   2. Fill in data on Sheet1
''   3. Run macro RunCut
'' ============================================================================

Option Explicit

'' == Constants ================================================================

Private Const SHEET_NAME       As String = "Sheet1"
Private Const PIPE_NAME        As String = "\\.\pipe\cut_progress"
Private Const GENERIC_READ     As Long   = &H80000000
Private Const OPEN_EXISTING    As Long   = 3
Private Const FILE_ATTR_NORMAL As Long   = &H80
Private Const BUFFER_SIZE      As Long   = 8192

Private Const SHEET_W_CELL     As String = "H1"  ' sheet width (mm)
Private Const SHEET_H_CELL     As String = "I1"  ' sheet height (mm)
Private Const KERF_CELL        As String = "I2"  ' blade kerf width (mm)
Private Const MARGIN_CELL      As String = "I3"  ' edge margin (mm)
Private Const LABEL_PCT_CELL   As String = "I4"  ' label size as % of minor block dimension (default 8)
Private Const LABEL_TXT_MIN    As Long   = 80    ' label text height lower bound at labelPct=100 (scales with labelPct)
Private Const LABEL_TXT_MAX    As Long   = 100   ' label text height upper bound at labelPct=100 (scales with labelPct)
Private Const DATA_CELL        As String = "A5"  ' top-left of piece table ("Panel" label column, first input row)
Private Const RESULT_CELL      As String = "M7"  ' top-left of placement table ("Sheet" label column, first result row)

Private Const CFG_RANDOM_SEED_CHK As String = "ChkRandomSeed"  ' checkbox: randomize seed on each run

Private Const CFG_SEED_CELL    As String = "L1"  ' base random seed (--seed)
Private Const CFG_THREADS_CELL As String = "L2"  ' parallel threads (--threads)
Private Const CFG_GENS_CELL    As String = "L3"  ' generations per run (--gens)
Private Const CFG_POP_CELL     As String = "L4"  ' population size (--pop)
Private Const OUT_STATUS_CELL  As String = "P1"  ' status text
Private Const OUT_GEN_CELL     As String = "P2"  ' current generation
Private Const OUT_OBJ_CELL     As String = "P3"  ' best objective
Private Const OUT_SHEETS_CELL  As String = "P4"  ' sheets used

Private Const CANVAS_RANGE     As String = "G6:L6"  ' top row of canvas; left col = draw origin, right col = width boundary
Private Const CANVAS_SHEET_GAP As Double = 14#   ' gap between sheets in points
Private Const SHEET_GAP_ACAD   As Long   = 150   ' gap between sheets exported to AutoCAD (drawing units)
#If VBA7 Then
    Private Const INVALID_HANDLE As LongPtr = -1
#Else
    Private Const INVALID_HANDLE As Long = -1
#End If

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

'' == State ====================================================================

Private g_Running As Boolean

'' == Helpers ==================================================================

' Escapes a string for JSON: non-ASCII and control chars become \uXXXX.
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

Private Function DataCol(ws As Worksheet) As Long
    DataCol = ws.Range(DATA_CELL).Column
End Function

Private Function DataHeaderRow(ws As Worksheet) As Long
    DataHeaderRow = ws.Range(DATA_CELL).Row
End Function

Private Function DataStartRow(ws As Worksheet) As Long
    DataStartRow = ws.Range(DATA_CELL).Row + 1
End Function

' Writes progress values to status cells.
Private Sub SetProgress(ws As Worksheet, status As String, gen As String, _
                        obj As String, sheets As String)
    ws.Range(OUT_STATUS_CELL).Value = status
    If gen    <> "" Then ws.Range(OUT_GEN_CELL).Value    = CLng(gen)
    If obj    <> "" Then ws.Range(OUT_OBJ_CELL).Value    = CDbl(obj)
    If sheets <> "" Then ws.Range(OUT_SHEETS_CELL).Value = CLng(sheets)
    DoEvents
End Sub

' Writes progress labels (column to the left of status cells) and clears previous results.
Private Sub InitOutputArea(ws As Worksheet)
    ws.Range(ws.Range(RESULT_CELL), ws.Cells(1000, ws.Range(RESULT_CELL).Column + 6)).ClearContents
    ws.Range(OUT_OBJ_CELL).NumberFormat = "# ### ### ##0"
    ClearLayoutShapes ws
    ClearPieceColors ws
End Sub

' Renders the placement table after a Done message.
Private Sub RenderPlacements(ws As Worksheet, sol As Object, pieces As Object)
    Dim rRow As Long:  rRow  = ws.Range(RESULT_CELL).Row
    Dim rCol As Long:  rCol  = ws.Range(RESULT_CELL).Column  ' "Sheet" label column
    Dim r    As Long:  r     = rRow

    ' Headers
    ws.Cells(r, rCol).Value     = "Sheet"
    ws.Cells(r, rCol + 1).Value = "Piece"
    ws.Cells(r, rCol + 2).Value = "X"
    ws.Cells(r, rCol + 3).Value = "Y"
    ws.Cells(r, rCol + 4).Value = "Width"
    ws.Cells(r, rCol + 5).Value = "Height"
    ws.Cells(r, rCol + 6).Value = "Rotated"
    r = r + 1

    Dim pl As Object
    For Each pl In sol("placements")
        Dim idx As Long
        idx = pl("piespec_idx") + 1  ' VBA Collection is 1-based

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

        ws.Cells(r, rCol).Value     = pl("sheet_idx")
        ws.Cells(r, rCol + 1).Value = pieceName
        ws.Cells(r, rCol + 2).Value = pl("x")
        ws.Cells(r, rCol + 3).Value = pl("y")
        ws.Cells(r, rCol + 4).Value = pw
        ws.Cells(r, rCol + 5).Value = ph
        ws.Cells(r, rCol + 6).Value = IIf(pl("rotated"), "yes", "")
        r = r + 1
    Next pl
    DoEvents
End Sub

'' == Layout drawing ===========================================================

' Returns a deterministic fill color from a 12-color palette by row index (0-based).
Private Function PieceColor(ByVal colorIdx As Long) As Long
    Dim p(11) As Long
    p(0)  = RGB(255, 182, 193): p(1)  = RGB(173, 216, 230)
    p(2)  = RGB(144, 238, 144): p(3)  = RGB(255, 255, 153)
    p(4)  = RGB(255, 200, 120): p(5)  = RGB(221, 160, 221)
    p(6)  = RGB(135, 206, 235): p(7)  = RGB(240, 180, 180)
    p(8)  = RGB(180, 255, 180): p(9)  = RGB(255, 228, 196)
    p(10) = RGB(200, 200, 255): p(11) = RGB(255, 240, 180)
    PieceColor = p(colorIdx Mod 12)
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

' Draws cutting layout as Excel shapes; origin and width taken from CANVAS_RANGE.
' Sheets are displayed side-by-side; pieces are color-coded by name.
Private Sub DrawLayout(ws As Worksheet, sol As Object, pieces As Object, _
                       sheetW As Long, sheetH As Long)
    If sheetW <= 0 Or sheetH <= 0 Then Exit Sub

    ClearLayoutShapes ws

    Dim cvs         As Range:  Set cvs     = ws.Range(CANVAS_RANGE)
    Dim originLeft  As Double: originLeft  = cvs.Cells(1, 1).Left
    Dim originTop   As Double: originTop   = cvs.Cells(1, 1).Top
    Dim sheetsDispW As Double: sheetsDispW = cvs.Cells(1, cvs.Columns.Count).Left - originLeft
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
        bg.TextFrame.Characters.Text = "Sheet " & si
        bg.TextFrame.Characters.Font.Size = 8
    Next si

    ' Draw pieces
    Dim pIdx As Long: pIdx = 0
    For Each pl In sol("placements")
        Dim idx   As Long: idx   = pl("piespec_idx") + 1
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
        s.Name = "cut_p_" & pIdx
        pIdx = pIdx + 1
        s.Fill.ForeColor.RGB = PieceColor(idx - 1)
        s.Fill.Transparency = 0
        s.Line.ForeColor.RGB = RGB(80, 80, 80)
        s.Line.Weight = 0.5

        If rWidth >= 20# And rHeight >= 12# Then
            s.TextFrame.Characters.Text = pieces(idx)("name")
            s.TextFrame.Characters.Font.Size = 7
            s.TextFrame.Characters.Font.Color = vbBlack
            s.TextFrame.HorizontalAlignment = xlHAlignCenter
            s.TextFrame.VerticalAlignment = xlVAlignCenter
        Else
            s.TextFrame.Characters.Text = ""
        End If
    Next pl
End Sub

'' == Piece row coloring =======================================================

Private Sub ClearPieceColors(ws As Worksheet)
    ws.Range(ws.Cells(DataStartRow(ws), DataCol(ws)), ws.Cells(200, DataCol(ws) + 4)).Interior.ColorIndex = xlNone
End Sub

Private Sub ColorPieceRows(ws As Worksheet, pieces As Object)
    Dim dc As Long: dc = DataCol(ws)
    Dim ci As Long: ci = 0
    Dim i As Long
    For i = DataStartRow(ws) To 200
        If ws.Cells(i, dc + 1).Value = "" Then Exit For
        ws.Range(ws.Cells(i, dc), ws.Cells(i, dc + 4)).Interior.Color = PieceColor(ci)
        ci = ci + 1
    Next i
End Sub

'' == JSON builder =============================================================

Private Function BuildProblemJson(ws As Worksheet) As String
    Dim sheetWidth  As Long: sheetWidth  = ws.Range(SHEET_W_CELL).Value
    Dim sheetHeight As Long: sheetHeight = ws.Range(SHEET_H_CELL).Value
    Dim kerf        As Long: kerf        = ws.Range(KERF_CELL).Value
    Dim margin      As Long: margin      = ws.Range(MARGIN_CELL).Value

    Dim dc As Long: dc = DataCol(ws)
    Dim sPieces As String
    Dim bFirst  As Boolean: bFirst = True
    Dim i As Long: i = DataStartRow(ws)

    Do
        Dim w As Long, h As Long
        w = 0: h = 0
        If ws.Cells(i, dc + 1).Value <> "" Then w = CLng(ws.Cells(i, dc + 1).Value)
        If ws.Cells(i, dc + 2).Value <> "" Then h = CLng(ws.Cells(i, dc + 2).Value)
        If w = 0 Or h = 0 Then Exit Do

        Dim pName   As String:  pName   = Trim(ws.Cells(i, dc).Value)
        Dim pCount  As Long:    pCount  = CLng(ws.Cells(i, dc + 3).Value)
        Dim pRotate As Boolean: pRotate = (ws.Cells(i, dc + 4).Value = True)

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
                       ",""margin"":" & CStr(margin) & _
                       ",""piespecs"":[" & sPieces & "]}"
End Function

'' == Main macro ===============================================================

Public Sub RunCut()
    If g_Running Then
        MsgBox "Solver is already running!", vbInformation
        Exit Sub
    End If

    Dim ws As Worksheet
    Set ws = ThisWorkbook.sheets(SHEET_NAME)

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
    ' nThreads = 0 - auto-detect via available_parallelism()
    Dim nSeed    As Long: nSeed    = 42
    Dim nThreads As Long: nThreads = 0
    Dim nGens    As Long: nGens    = 2000
    Dim nPop     As Long: nPop     = 200
    If ws.CheckBoxes(CFG_RANDOM_SEED_CHK).Value = xlOn Then
        Randomize
        nSeed = Int(Rnd() * 10000)
        ws.Range(CFG_SEED_CELL).Value = nSeed
    ElseIf ws.Range(CFG_SEED_CELL).Value <> "" Then
        nSeed = CLng(ws.Range(CFG_SEED_CELL).Value)
    End If
    If ws.Range(CFG_THREADS_CELL).Value <> "" Then nThreads = CLng(ws.Range(CFG_THREADS_CELL).Value)
    If ws.Range(CFG_GENS_CELL).Value    <> "" Then nGens    = CLng(ws.Range(CFG_GENS_CELL).Value)
    If ws.Range(CFG_POP_CELL).Value     <> "" Then nPop     = CLng(ws.Range(CFG_POP_CELL).Value)

    ' Launch cut.exe (non-blocking Shell)
    Dim cmd As String
    cmd = Chr(34) & exePath & Chr(34) & " calc --json " & Chr(34) & tmpFile & Chr(34) _
        & " --seed " & nSeed & " --threads " & nThreads & " --gens " & nGens & " --pop " & nPop
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

            ' Last element from Split without trailing \n - incomplete line
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
                        CStr(msg("last_sheet_area")), _
                        CStr(msg("sheets_used"))

                Case "done"
                    SetProgress ws, "Done " & ChrW(10003), "", _
                        CStr(msg("last_sheet_area")), _
                        CStr(msg("sheets_used"))
                    Application.ScreenUpdating = False
                    ws.Range(ws.Range(RESULT_CELL), _
                             ws.Cells(1000, ws.Range(RESULT_CELL).Column + 6)).ClearContents
                    RenderPlacements ws, msg("solution"), msg("pieces")
                    DrawLayout ws, msg("solution"), msg("pieces"), _
                        ws.Range(SHEET_W_CELL).Value, ws.Range(SHEET_H_CELL).Value
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

Public Function IsRunning() As Boolean
    IsRunning = g_Running
End Function

Public Sub RestartCut()
    If g_Running Then
        StopCut
        Application.OnTime Now + TimeValue("00:00:01"), "Cut.RunCut"
    Else
        RunCut
    End If
End Sub

Public Sub StopCut()
    g_Running = False
    Dim ws As Worksheet
    Set ws = ThisWorkbook.sheets(SHEET_NAME)
    ws.Range(OUT_STATUS_CELL).Value = "Stopped"
    ws.Range(ws.Range(RESULT_CELL), ws.Cells(1000, ws.Range(RESULT_CELL).Column + 6)).ClearContents
    ClearLayoutShapes ws
    ClearPieceColors ws
End Sub

'' == AutoCAD export ==========================================================

' Returns a running AutoCAD instance, or Nothing if AutoCAD is not open.
' Tries the generic ProgID first (AutoCAD 2002), then the versioned one for 2015.
Private Function GetAcadInstance() As Object
    Dim acad As Object
    On Error Resume Next
    Set acad = GetObject(, "AutoCAD.Application")     ' AutoCAD 2002
    If acad Is Nothing Then _
        Set acad = GetObject(, "AutoCAD.Application.20")  ' AutoCAD 2015
    On Error GoTo 0
    Set GetAcadInstance = acad
End Function

' Returns the name of a text style using Tahoma TTF (creates it if absent).
' Tahoma is a TrueType font with full Unicode/Cyrillic glyph support.
' CharSet 204 = RUSSIAN_CHARSET - tells GDI to use the Cyrillic code page for hinting.
Private Function EnsureCutTextStyle(doc As Object) As String
    Const STYLE_NAME As String = "CUT_STYLE"
    Dim sty As Object
    On Error Resume Next
    Set sty = doc.TextStyles(STYLE_NAME)
    On Error GoTo 0
    If sty Is Nothing Then
        Set sty = doc.TextStyles.Add(STYLE_NAME)
        sty.SetFont "Tahoma", False, False, 204, 0
    End If
    EnsureCutTextStyle = STYLE_NAME
End Function

' Adds one or two centered AddText objects to blkDef labelling the piece.
' Two lines (labelDim on top/right, labelName on bottom/left) when minSide >= 100;
' single line ("labelDim labelName") otherwise.
Private Sub AddPieceLabel(blkDef As Object, bw As Long, bh As Long, _
                          labelDim As String, labelName As String, _
                          cutStyle As String, labelPct As Double)
    Dim minSide   As Long:    minSide   = IIf(bw < bh, bw, bh)
    Dim maxSide   As Long:    maxSide   = IIf(bw < bh, bh, bw)
    Dim rotated90 As Boolean: rotated90 = (bw < bh)

    Dim useTwoLines As Boolean: useTwoLines = (Len(labelName) > 0) And (minSide >= 100)

    Dim lbl1 As String, lbl2 As String, nChars As Long
    If useTwoLines Then
        lbl1 = labelDim
        lbl2 = "(" & labelName & ")"
        nChars = IIf(Len(lbl1) > Len(lbl2), Len(lbl1), Len(lbl2))
    ElseIf Len(labelName) > 0 Then
        lbl1 = labelDim & " (" & labelName & ")"
        nChars = Len(lbl1)
    Else
        lbl1 = labelDim
        nChars = Len(lbl1)
    End If

    ' Text height: minSide * labelPct/2, so I4=100 gives minSide*50%.
    ' Text is allowed to overflow the block boundary — readability beats containment.
    Dim txtH As Double
    txtH = CDbl(minSide) * labelPct / 2

    ' Clamp bounds scale with labelPct: at I4=100 -> [80,100]; at I4=50 -> [40,50]; at I4=200 -> [160,200].
    Dim txtMin As Double: txtMin = LABEL_TXT_MIN * labelPct
    Dim txtMax As Double: txtMax = LABEL_TXT_MAX * labelPct
    If txtH < txtMin Then txtH = txtMin
    If txtH > txtMax Then txtH = txtMax

    Dim cx As Double: cx = CDbl(bw) / 2
    Dim cy As Double: cy = CDbl(bh) / 2
    Dim off As Double: off = txtH * 0.65

    Dim pt1(0 To 2) As Double
    Dim pt2(0 To 2) As Double
    pt1(2) = 0: pt2(2) = 0
    If useTwoLines Then
        If rotated90 Then
            pt1(0) = cx - off: pt1(1) = cy
            pt2(0) = cx + off: pt2(1) = cy
        Else
            pt1(0) = cx: pt1(1) = cy + off
            pt2(0) = cx: pt2(1) = cy - off
        End If
    Else
        pt1(0) = cx: pt1(1) = cy
    End If

    Dim t As Object
    Set t = blkDef.AddText(lbl1, pt1, txtH)
    t.StyleName = cutStyle: t.Alignment = 4: t.TextAlignmentPoint = pt1
    If rotated90 Then t.Rotation = 1.5707963265
    Set t = Nothing

    If useTwoLines Then
        Set t = blkDef.AddText(lbl2, pt2, txtH)
        t.StyleName = cutStyle: t.Alignment = 4: t.TextAlignmentPoint = pt2
        If rotated90 Then t.Rotation = 1.5707963265
        Set t = Nothing
    End If
End Sub

Public Sub SendToAutoCAD()
    Dim ws As Worksheet
    Set ws = ThisWorkbook.sheets(SHEET_NAME)

    Dim shW      As Long:   shW      = ws.Range(SHEET_W_CELL).Value
    Dim shH      As Long:   shH      = ws.Range(SHEET_H_CELL).Value
    Dim kerf     As Long:   kerf     = ws.Range(KERF_CELL).Value
    Dim margin   As Long:   margin   = ws.Range(MARGIN_CELL).Value
    Dim labelPct As Double: labelPct = 0.08
    If ws.Range(LABEL_PCT_CELL).Value <> "" Then
        labelPct = CDbl(ws.Range(LABEL_PCT_CELL).Value) / 100#
    End If
    If shW = 0 Or shH = 0 Then
        MsgBox "Sheet dimensions not set (H1, I1).", vbExclamation: Exit Sub
    End If

    Dim rRow As Long: rRow = ws.Range(RESULT_CELL).Row + 1
    Dim rCol As Long: rCol = ws.Range(RESULT_CELL).Column
    If ws.Cells(rRow, rCol).Value = "" Then
        MsgBox "No placements. Run solver first.", vbExclamation: Exit Sub
    End If

    Dim acad As Object, doc As Object
    Set acad = GetAcadInstance()
    If acad Is Nothing Then
        MsgBox "AutoCAD not running. Open a drawing first.", vbCritical: Exit Sub
    End If
    Set doc = acad.ActiveDocument
    acad.Visible = True

    Dim cutStyle As String: cutStyle = EnsureCutTextStyle(doc)

    ' Clear all entities from model space (AOM - synchronous, no timing issues)
    Dim ms As Object: Set ms = doc.ModelSpace
    Dim i As Long
    For i = ms.Count - 1 To 0 Step -1
        ms.Item(i).Delete
    Next i

    ' Remove old cp* block definitions so names can be reused
    Dim blkNames() As String
    Dim nOld As Long: nOld = 0
    Dim blk As Object
    For Each blk In doc.Blocks
        If Left(blk.Name, 2) = "cp" Then
            ReDim Preserve blkNames(nOld)
            blkNames(nOld) = blk.Name
            nOld = nOld + 1
        End If
    Next blk
    For i = 0 To nOld - 1
        On Error Resume Next
        doc.Blocks(blkNames(i)).Delete
        On Error GoTo 0
    Next i

    ' Build one block per placement and insert it via AOM
    Dim r As Long: Dim pIdx As Long: pIdx = 0
    For r = rRow To 10000
        If ws.Cells(r, rCol).Value = "" And ws.Cells(r, rCol + 1).Value = "" Then Exit For

        Dim shIdx As Long: shIdx = CLng(ws.Cells(r, rCol).Value)       ' sheet index (0-based)
        Dim px As Long: px = CLng(ws.Cells(r, rCol + 2).Value)          ' X from sheet left (Y-down coords)
        Dim py As Long: py = CLng(ws.Cells(r, rCol + 3).Value)          ' Y from sheet top  (Y-down coords)
        Dim pw As Long: pw = CLng(ws.Cells(r, rCol + 4).Value)          ' placed width
        Dim ph As Long: ph = CLng(ws.Cells(r, rCol + 5).Value)          ' placed height
        ' Rotated layout: solver-X (width) maps to AutoCAD-Y, solver-Y (height) maps to AutoCAD-X.
        ' Sheet occupies [xOff, xOff+shH] x [0, shW] in drawing space.
        Dim xOff As Long: xOff = shIdx * (shH + SHEET_GAP_ACAD)        ' sheet left edge in drawing

        ' Create block definition (origin at 0,0)
        Dim bName As String: bName = "cp" & pIdx
        Dim basePt(0 To 2) As Double  ' (0,0,0)
        Dim blkDef As Object
        Set blkDef = doc.Blocks.Add(basePt, bName)

        ' Block space: (ph+kerf) wide (AutoCAD X = solver Y dir), (pw+kerf) tall (AutoCAD Y = solver X dir)
        ' Expanding by kerf makes adjacent blocks snap flush — no manual kerf offset needed.
        Dim bw As Long: bw = ph + kerf  ' block width  in AutoCAD X
        Dim bh As Long: bh = pw + kerf  ' block height in AutoCAD Y
        Dim rPts(0 To 7) As Double
        rPts(0) = 0:  rPts(1) = 0
        rPts(2) = bw: rPts(3) = 0
        rPts(4) = bw: rPts(5) = bh
        rPts(6) = 0:  rPts(7) = bh
        Dim rectObj As Object
        Set rectObj = blkDef.AddLightWeightPolyline(rPts)
        rectObj.Closed = True

        ' AutoCAD 2002 (version 15.x) cannot render the Unicode × (cross) sign - use plain "x".
        Dim dimSep    As String: dimSep    = IIf(Val(acad.Version) < 16, "x", ChrW(215))
        Dim labelDim  As String: labelDim  = CStr(pw) & dimSep & CStr(ph)
        Dim labelName As String: labelName = CStr(ws.Cells(r, rCol + 1).Value)
        AddPieceLabel blkDef, bw, bh, labelDim, labelName, cutStyle, labelPct

        ' Insert point = bottom-left of block in drawing space
        ' acad_x = xOff + py,  acad_y = shW - px - pw
        Dim insPt(0 To 2) As Double
        insPt(0) = xOff + py
        insPt(1) = shW - px - pw
        insPt(2) = 0
        ms.InsertBlock insPt, bName, 1, 1, 1, 0

        Set rectObj = Nothing: Set blkDef = Nothing
        pIdx = pIdx + 1
    Next r

    ' Draw sheet outlines
    Dim nSheets As Long: nSheets = CLng(ws.Range(OUT_SHEETS_CELL).Value)
    If nSheets < 1 Then nSheets = 1
    Dim si As Long
    For si = 0 To nSheets - 1
        Dim sxOff As Long: sxOff = si * (shH + SHEET_GAP_ACAD)
        Dim sPts(0 To 7) As Double
        sPts(0) = sxOff:                  sPts(1) = 0
        sPts(2) = sxOff + shH + kerf:     sPts(3) = 0
        sPts(4) = sxOff + shH + kerf:     sPts(5) = shW + kerf
        sPts(6) = sxOff:                  sPts(7) = shW + kerf
        Dim shRect As Object
        Set shRect = ms.AddLightWeightPolyline(sPts)
        shRect.Closed = True

        If margin > 0 Then
            ' Inner rectangle — working area after margin
            Dim iPts(0 To 7) As Double
            iPts(0) = sxOff + margin:                iPts(1) = margin
            iPts(2) = sxOff + shH + kerf - margin:   iPts(3) = margin
            iPts(4) = sxOff + shH + kerf - margin:   iPts(5) = shW + kerf - margin
            iPts(6) = sxOff + margin:                iPts(7) = shW + kerf - margin
            Dim innerRect As Object
            Set innerRect = ms.AddLightWeightPolyline(iPts)
            innerRect.Closed = True

            ' Hatch the margin band between outer and inner rectangles
            Dim ht As Object
            Set ht = ms.AddHatch(0, "ANSI31", False)
            Dim outerLoop(0) As Object: Set outerLoop(0) = shRect
            Dim innerLoop(0) As Object: Set innerLoop(0) = innerRect
            ht.AppendOuterLoop outerLoop
            ht.AppendInnerLoop innerLoop
            ht.PatternScale = CDbl(margin) / 2
            ht.Color = 8  ' gray
            ht.Evaluate
            Set ht = Nothing
            Set innerRect = Nothing
        End If

        Set shRect = Nothing
    Next si

    doc.SendCommand "ZOOM" & vbCr & "e" & vbCr
End Sub

'' == Checkboxes for "Can rotate?" =============================================

Sub CreateCheckboxes()
    Dim ws As Worksheet
    Set ws = ThisWorkbook.sheets(SHEET_NAME)

    Dim shp As Object
    For Each shp In ws.CheckBoxes
        If shp.Name <> CFG_RANDOM_SEED_CHK Then shp.Delete
    Next shp

    Dim cbCol     As Long:   cbCol     = DataCol(ws) + 5
    Dim colWidth  As Double: colWidth  = ws.Columns(cbCol).Width
    Dim cell      As Range
    Dim cb        As CheckBox
    Dim i         As Integer

    Dim mg As Double: mg = 1#

    Set cell = ws.Cells(DataHeaderRow(ws), cbCol)
    Set cb = ws.CheckBoxes.Add(cell.Left + mg, cell.Top + mg, colWidth - 2*mg, cell.Height - 2*mg)
    cb.Caption  = "all"
    cb.OnAction = "Cut.MainCheckboxClick"
    cb.Name     = "cbMain"

    For i = DataStartRow(ws) To DataStartRow(ws) + 99
        Set cell = ws.Cells(i, cbCol)
        Set cb = ws.CheckBoxes.Add(cell.Left + mg, cell.Top + mg, colWidth - 2*mg, cell.Height - 2*mg)
        cb.LinkedCell = ws.Cells(i, DataCol(ws) + 4).Address
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

    Dim dc As Long: dc = DataCol(ws)
    For i = DataStartRow(ws) To DataStartRow(ws) + 99
        If ws.Cells(i, dc + 4).Value <> "" Then
            ws.Cells(i, dc + 4).Value = mainVal
        End If
    Next i
End Sub
